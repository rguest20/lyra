"""JSON-lines MLX worker. Stdout is reserved for protocol responses."""
import json
import math
import os
import sys
from pathlib import Path


def send(value):
    print(json.dumps(value, allow_nan=False), flush=True)


def memory_stats(mx):
    return {'active_bytes': mx.get_active_memory(), 'peak_bytes': mx.get_peak_memory(), 'cache_bytes': mx.get_cache_memory()}


def main():
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim
    from mlx_lm.generate import generate_step
    from mlx_lm.models.qwen3_5 import Model, ModelArgs
    from mlx_lm.sample_utils import make_sampler

    init = json.loads(sys.argv[1])
    config = json.loads((Path(init['model_dir']) / 'config.json').read_text())
    if config.get('quantization') or config.get('quantization_config'):
        raise ValueError('Quantized checkpoints are not supported by this BF16 backend')
    base = Model(ModelArgs.from_dict(config))
    weights = {}
    for checkpoint in init['checkpoints']:
        weights.update(mx.load(checkpoint))
    base.load_weights(list(base.sanitize(weights).items()), strict=True)
    del weights
    base.eval()
    base.freeze()
    mx.eval(base.parameters())

    class AdapterModel(nn.Module):
        def __init__(self):
            super().__init__()
            self.base = base
            args = base.language_model.args
            self.down = mx.random.normal((args.hidden_size, 8)).astype(mx.float32) * 0.01
            self.up = mx.zeros((8, args.vocab_size), dtype=mx.float32)
            self.adapter_active = False

        def baseline(self, hidden):
            lm = self.base.language_model
            return lm.model.embed_tokens.as_linear(hidden) if lm.args.tie_word_embeddings else lm.lm_head(hidden)

        def __call__(self, inputs, cache=None, **kwargs):
            hidden = self.base.language_model.model(inputs, cache=cache, **kwargs)
            baseline = self.baseline(hidden)
            if not self.adapter_active:
                return baseline
            return baseline.astype(mx.float32) + hidden.astype(mx.float32) @ self.down @ self.up

        def make_cache(self):
            return self.base.make_cache()

        @property
        def layers(self):
            return self.base.layers

    model = AdapterModel()
    adapter = Path(init['adapter'])
    adapter_loaded = adapter.exists()
    training_steps = 0
    if adapter_loaded:
        saved, metadata = mx.load(str(adapter), return_metadata=True)
        training_steps = int(metadata['training_steps']) if 'training_steps' in metadata else None
        for name in ('down', 'up'):
            if name not in saved or saved[name].shape != getattr(model, name).shape:
                raise ValueError('Adapter belongs to a different model shape; select a fresh adapter')
            setattr(model, name, saved[name].astype(mx.float32))
    model.eval()
    mx.eval(model.down, model.up)
    model.adapter_active = bool(mx.any(model.up != 0).item())

    def save_adapter(path):
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.stem + '.tmp.safetensors')
        metadata = {'model_type': 'qwen3_5', 'adapter_kind': 'output_rank8'}
        if training_steps is not None:
            metadata['training_steps'] = str(training_steps)
        mx.save_safetensors(str(temporary), {'down': model.down, 'up': model.up}, metadata=metadata)
        os.replace(temporary, path)

    if not adapter_loaded:
        save_adapter(adapter)
    send({'ready': True, 'backend': 'mlx', 'precision': 'BF16', 'adapter_loaded': adapter_loaded,
          'adapter_created': not adapter_loaded, 'adapter_active': model.adapter_active,
          'adapter_training_steps': training_steps, 'memory': memory_stats(mx)})

    for line in sys.stdin:
        try:
            request = json.loads(line)
            op = request['op']
            if op == 'generate':
                stops = set(request['stop_ids'])
                tokens = []
                finish_reason = 'max_tokens'
                in_thinking = request.get('thinking', False)
                thought_tokens = 0
                mx.reset_peak_memory()
                sampler = make_sampler(temp=request['temperature'], top_p=0.8, top_k=20)
                generator = generate_step(mx.array(request['ids']), model, max_tokens=request['max_tokens'], sampler=sampler, prefill_step_size=128)
                try:
                    for token, _ in generator:
                        token = int(token.item()) if hasattr(token, 'item') else int(token)
                        if token in stops:
                            finish_reason = 'stop_token'
                            break
                        tokens.append(token)
                        if token == request.get('think_start_id'):
                            in_thinking = True
                        if token == request.get('think_end_id'):
                            in_thinking = False
                        if in_thinking:
                            thought_tokens += 1
                            if thought_tokens >= request.get('thinking_budget', 256):
                                finish_reason = 'thinking_budget'
                                break
                finally:
                    generator.close()
                send({'tokens': tokens, 'finish_reason': finish_reason, 'generated_tokens': len(tokens), 'thinking_tokens': thought_tokens, 'memory': memory_stats(mx)})
            elif op == 'train':
                targets = request['targets']
                sequence = request['ids'] + targets[:-1]
                hidden = model.base.language_model.model(mx.array([sequence]))[:, len(request['ids']) - 1:, :]
                hidden = mx.stop_gradient(hidden)
                baseline = mx.stop_gradient(model.baseline(hidden).astype(mx.float32))
                hidden = hidden.astype(mx.float32)
                labels = mx.array([targets])
                mx.eval(hidden, baseline)

                def loss_fn(params):
                    logits = baseline + hidden @ params['down'] @ params['up']
                    return nn.losses.cross_entropy(logits, labels, reduction='mean')

                params = {'down': model.down, 'up': model.up}
                loss, gradients = mx.value_and_grad(loss_fn)(params)
                optimizer = optim.AdamW(learning_rate=0.01)
                params = optimizer.apply_gradients(gradients, params)
                mx.eval(loss, params)
                numeric_loss = float(loss.item())
                if not math.isfinite(numeric_loss):
                    raise ValueError('Training produced a nonfinite loss')
                model.down, model.up = params['down'], params['up']
                model.adapter_active = True
                if training_steps is not None:
                    training_steps += 1
                save_adapter(Path(request['adapter']))
                send({'tokens': len(targets), 'loss': numeric_loss})
            elif op == 'merge':
                destination = Path(request['destination'])
                if destination.exists():
                    raise ValueError('Destination checkpoint already exists')
                weights = {}
                for checkpoint in request['checkpoints']:
                    weights.update(mx.load(checkpoint))
                head_name = 'lm_head.weight'
                embedding_name = 'model.language_model.embed_tokens.weight'
                original = weights.get(head_name, weights.get(embedding_name))
                if original is None or original.dtype != mx.bfloat16:
                    raise ValueError('Checkpoint output head must be BF16')
                delta = (model.down @ model.up).T
                weights[head_name] = (original.astype(mx.float32) + delta).astype(mx.bfloat16)
                destination.parent.mkdir(parents=True, exist_ok=True)
                temporary = destination.with_name(destination.stem + '.tmp.safetensors')
                if temporary.exists():
                    raise ValueError('Temporary checkpoint already exists')
                mx.save_safetensors(str(temporary), weights)
                os.replace(temporary, destination)
                send({'saved': True})
            else:
                raise ValueError('Unknown MLX worker operation')
            mx.clear_cache()
        except Exception as error:
            mx.clear_cache()
            send({'error': f'{type(error).__name__}: {error}', 'memory': memory_stats(mx)})


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        send({'error': f'{type(error).__name__}: {error}'})
        sys.exit(1)
