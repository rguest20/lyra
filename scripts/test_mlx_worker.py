"""Generation protocol checks; no model or semantic-accuracy claims."""
import unittest

from mlx_worker import (PrefixCache, TokenTracker, common_prefix_length,
                        make_adapter_model, request_logits_processors)


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

    def test_prompt_length_override_offsets_from_the_cached_suffix(self):
        history = []
        request = {'ids': [3, 4, 5, 6], 'presence_exclude_prompt': True}
        processors = request_logits_processors(
            request, lambda **_: [lambda tokens, logits: history.append(tokens)], prompt_length=1)
        processors[0]([6, 1, 2], None)
        self.assertEqual(history, [[1, 2]])


class TokenTrackerTests(unittest.TestCase):
    REQUEST = {'stop_ids': [9], 'think_start_id': 7, 'think_end_id': 8, 'thinking_budget': 2}

    def test_stop_token_is_not_recorded_and_ends_generation(self):
        tracker = TokenTracker(self.REQUEST)
        self.assertTrue(tracker.accept(1))
        self.assertFalse(tracker.accept(9))
        self.assertEqual(tracker.result(), {'tokens': [1], 'finish_reason': 'stop_token',
                                            'generated_tokens': 1, 'thinking_tokens': 0})

    def test_thinking_budget_counts_tokens_between_think_markers(self):
        tracker = TokenTracker(self.REQUEST)
        self.assertTrue(tracker.accept(7))
        self.assertFalse(tracker.accept(1))
        self.assertEqual(tracker.finish_reason, 'thinking_budget')
        self.assertEqual(tracker.thinking_tokens, 2)

    def test_closing_think_marker_stops_budget_counting(self):
        tracker = TokenTracker(dict(self.REQUEST, thinking=True, thinking_budget=5))
        for token in (1, 8, 2, 3, 4, 5):
            self.assertTrue(tracker.accept(token))
        # Only the token before the closing marker is counted against the budget.
        self.assertEqual(tracker.thinking_tokens, 1)
        self.assertEqual(tracker.finish_reason, 'max_tokens')


class FakeKV:
    def __init__(self, offset=0):
        self.offset = offset

    def trim(self, n):
        n = min(self.offset, n)
        self.offset -= n
        return n


class FakeModel:
    def __init__(self):
        self.created = 0

    def make_cache(self):
        self.created += 1
        return [FakeKV(), FakeKV()]


def fake_trim(cache, n):
    return [c.trim(n) for c in cache][0]


class PrefixCacheTests(unittest.TestCase):
    def make(self, trimmable=True):
        model = FakeModel()
        cache = PrefixCache(model, lambda _: trimmable, fake_trim)
        cache.MIN_REUSE_TOKENS = 2
        return model, cache

    def test_common_prefix_length(self):
        self.assertEqual(common_prefix_length([1, 2, 3], [1, 2, 4]), 2)
        self.assertEqual(common_prefix_length([], [1]), 0)

    def test_first_request_builds_a_fresh_cache(self):
        model, cache = self.make()
        kv, prompt, reused = cache.prepare([1, 2, 3])
        self.assertEqual((prompt, reused, model.created), ([1, 2, 3], 0, 1))

    def test_shared_prefix_trims_cache_and_feeds_only_the_suffix(self):
        model, cache = self.make()
        kv, _, _ = cache.prepare([1, 2, 3])
        for c in kv:
            c.offset = 6  # prompt + 3 generated tokens, one of them prefetched
        cache.record([1, 2, 3], [10, 11])
        kv2, prompt, reused = cache.prepare([1, 2, 3, 10, 11, 4, 5])
        self.assertIs(kv2, kv)
        self.assertEqual((prompt, reused), ([4, 5], 5))
        self.assertEqual([c.offset for c in kv], [5, 5])
        self.assertEqual(model.created, 1)

    def test_identical_prompt_keeps_one_token_to_process(self):
        model, cache = self.make()
        kv, _, _ = cache.prepare([1, 2, 3, 4])
        for c in kv:
            c.offset = 4
        cache.record([1, 2, 3, 4], [])
        _, prompt, reused = cache.prepare([1, 2, 3, 4])
        self.assertEqual((prompt, reused), ([4], 3))

    def test_untrimmable_or_disallowed_caches_are_rebuilt(self):
        model, cache = self.make(trimmable=False)
        cache.prepare([1, 2, 3])
        cache.record([1, 2, 3], [4])
        _, prompt, reused = cache.prepare([1, 2, 3, 4, 5])
        self.assertEqual((prompt, reused, model.created), ([1, 2, 3, 4, 5], 0, 2))
        model, cache = self.make()
        cache.prepare([1, 2, 3])
        cache.record([1, 2, 3], [4])
        _, prompt, reused = cache.prepare([1, 2, 3, 4, 5], allow_reuse=False)
        self.assertEqual((prompt, reused, model.created), ([1, 2, 3, 4, 5], 0, 2))

    def test_short_shared_prefix_is_not_worth_reusing(self):
        model, cache = self.make()
        cache.MIN_REUSE_TOKENS = 16
        cache.prepare([1, 2, 3])
        cache.record([1, 2, 3], [])
        _, prompt, reused = cache.prepare([1, 2, 3, 4])
        self.assertEqual((reused, model.created), (0, 2))


if __name__ == '__main__':
    unittest.main()
