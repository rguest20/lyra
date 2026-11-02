"""Generation protocol checks; no model or semantic-accuracy claims."""
import unittest

from mlx_worker import make_adapter_model, request_logits_processors


class AdapterLayoutTests(unittest.TestCase):
    def test_quantized_qwen3_preserves_logits_and_incremental_cache(self):
        import mlx.core as mx
        import mlx.nn as nn
        from mlx_lm.models.qwen3 import Model, ModelArgs

        for tied in (False, True):
            with self.subTest(tied=tied):
                base = Model(ModelArgs(
                    model_type='qwen3', hidden_size=64, intermediate_size=128,
                    num_hidden_layers=2, num_attention_heads=4, num_key_value_heads=2,
                    head_dim=16, vocab_size=128, max_position_embeddings=64,
                    rms_norm_eps=1e-6, rope_theta=10000, tie_word_embeddings=tied))
                nn.quantize(base, group_size=32, bits=4)
                base.eval()
                model = make_adapter_model(base, 'qwen3')
                self.check_logits_and_cache(base, model)

    def test_qwen35_preserves_hybrid_cache_and_logits(self):
        from mlx_lm.models.qwen3_5 import Model, ModelArgs

        base = Model(ModelArgs(model_type='qwen3_5', text_config={
            'model_type': 'qwen3_5_text', 'hidden_size': 64, 'intermediate_size': 128,
            'num_hidden_layers': 2, 'num_attention_heads': 4, 'num_key_value_heads': 2,
            'head_dim': 16, 'vocab_size': 128, 'max_position_embeddings': 64,
            'linear_num_value_heads': 4, 'linear_num_key_heads': 2,
            'linear_key_head_dim': 16, 'linear_value_head_dim': 16,
            'full_attention_interval': 2,
            'rope_parameters': {'type': 'default', 'rope_theta': 10000,
                                'partial_rotary_factor': 0.5}}))
        base.eval()
        model = make_adapter_model(base, 'qwen3_5')
        self.check_logits_and_cache(base, model)
        self.assertEqual(type(model.make_cache()[0]).__name__, 'ArraysCache')
        self.assertEqual(type(model.make_cache()[1]).__name__, 'KVCache')

    def check_logits_and_cache(self, base, model):
        import mlx.core as mx
        from mlx_lm.models.cache import make_prompt_cache

        tokens = mx.array([[1, 2, 3]])
        self.assertTrue(mx.allclose(model(tokens), base(tokens)).item())
        cache = model.make_cache()
        baseline_cache = make_prompt_cache(base)
        self.assertEqual(len(cache), len(base.layers))
        self.assertTrue(mx.allclose(model(tokens, cache=cache),
                                    base(tokens, cache=baseline_cache)).item())
        token = mx.array([[4]])
        self.assertTrue(mx.allclose(model(token, cache=cache),
                                    base(token, cache=baseline_cache)).item())
        model.up = mx.ones_like(model.up) * 0.01
        model.adapter_active = True
        expected = base(tokens).astype(mx.float32) + model.hidden(tokens).astype(mx.float32) @ model.down @ model.up
        self.assertTrue(mx.allclose(model(tokens), expected).item())


class SamplingProtocolTests(unittest.TestCase):
    def test_coding_penalty_does_not_penalize_prompt_or_schema_tokens(self):
        import mlx.core as mx
        from mlx_lm.sample_utils import make_logits_processors

        request = {'ids': [3, 4], 'presence_penalty': 2.0,
                   'presence_context_size': 100, 'presence_exclude_prompt': True}
        processors = request_logits_processors(request, make_logits_processors)
        logits = mx.zeros((1, 8))
        for process in processors:
            logits = process(mx.array([3, 4, 1, 2]), logits)
        self.assertEqual(logits.tolist()[0], [0, -2, -2, 0, 0, 0, 0, 0])

    def test_initial_coding_token_has_no_generated_history_penalty(self):
        import mlx.core as mx
        from mlx_lm.sample_utils import make_logits_processors

        request = {'ids': [3, 4], 'presence_penalty': 2.0,
                   'presence_context_size': 100, 'presence_exclude_prompt': True}
        logits = mx.zeros((1, 8))
        for process in request_logits_processors(request, make_logits_processors):
            logits = process(mx.array([3, 4]), logits)
        self.assertEqual(logits.tolist()[0], [0] * 8)

    def test_existing_requests_keep_unmodified_processor_history(self):
        history = []

        def factory(**kwargs):
            self.assertIsNone(kwargs['presence_penalty'])
            self.assertEqual(kwargs['presence_context_size'], 20)
            return [lambda tokens, logits: history.append(tokens)]

        request_logits_processors({'ids': [3, 4]}, factory)[0]([3, 4, 1], None)
        self.assertEqual(history, [[3, 4, 1]])


if __name__ == '__main__':
    unittest.main()
