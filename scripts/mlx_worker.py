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


def request_logits_processors(request, factory, prompt_length=None):
    processors = factory(
        presence_penalty=request.get('presence_penalty'),
        presence_context_size=request.get('presence_context_size', 20))
    if not request.get('presence_exclude_prompt', False):
        return processors
    # MLX passes prompt + generated tokens to processors. Presence penalties
    # for coding apply to generated output, not required schema/input tokens.
    offset = len(request['ids']) if prompt_length is None else prompt_length
    return [lambda tokens, logits, process=process: process(tokens[offset:], logits)
            for process in processors]


def request_sampler(request, make_sampler):
    return make_sampler(temp=request['temperature'], top_p=request.get('top_p', 0.8), top_k=20)


class TokenTracker:
    """Apply the stop-token and thinking-budget rules of one request to a token stream."""

    def __init__(self, request):
        self.stops = set(request['stop_ids'])
        self.think_start = request.get('think_start_id')
        self.think_end = request.get('think_end_id')
        self.budget = request.get('thinking_budget', 256)
        self.thinking = request.get('thinking', False)
        self.tokens = []
        self.thinking_tokens = 0
        self.finish_reason = 'max_tokens'

    def accept(self, token):
        """Record one token; return False once this request must stop generating."""
        if token in self.stops:
            self.finish_reason = 'stop_token'
            return False
        self.tokens.append(token)
        if token == self.think_start:
            self.thinking = True
        if token == self.think_end:
            self.thinking = False
        if self.thinking:
            self.thinking_tokens += 1
            if self.thinking_tokens >= self.budget:
                self.finish_reason = 'thinking_budget'
                return False
        return True

    def result(self):
        return {'tokens': self.tokens, 'finish_reason': self.finish_reason,
                'generated_tokens': len(self.tokens), 'thinking_tokens': self.thinking_tokens}


def common_prefix_length(left, right):
    count = 0
    for a, b in zip(left, right):
        if a != b:
            break
        count += 1
    return count


class PrefixCache:
    """Reuse the previous request's KV cache for the prompt prefix it shares.

    Only trimmable caches (plain attention layers) are reused; hybrid layouts
    with recurrent state are always rebuilt. The cache contents are a function
    of the base weights alone, so adapter training does not invalidate them.
    """

    MIN_REUSE_TOKENS = 16

    def __init__(self, model, can_trim, trim):
        self.model = model
        self.can_trim = can_trim
        self.trim = trim
        self.ids = []
        self.cache = None

    def prepare(self, ids, allow_reuse=True):
        """Return (cache, prompt_suffix, reused_tokens) for a prompt."""
        reusable = (allow_reuse and self.cache is not None and self.can_trim(self.cache))
        if reusable:
            # Keep at least one prompt token so the model has input to process.
            common = min(common_prefix_length(self.ids, ids), len(ids) - 1)
            if common >= self.MIN_REUSE_TOKENS:
                stored = max(c.offset for c in self.cache if hasattr(c, 'offset'))
                self.trim(self.cache, stored - common)
                self.ids = self.ids[:common]
                return self.cache, ids[common:], common
        self.cache = self.model.make_cache()
        self.ids = []
        return self.cache, list(ids), 0

    def record(self, ids, tokens):
        """Remember which tokens the cache now covers, in order."""
        self.ids = list(ids) + list(tokens)

    def invalidate(self):
        self.cache = None
        self.ids = []


def make_adapter_model(base, model_type):
    """Share the output adapter across supported MLX language-model layouts."""
    import mlx.core as mx
    import mlx.nn as nn
    from mlx_lm.models.cache import make_prompt_cache

    if model_type == 'qwen3_5':
        language_model = base.language_model
    elif model_type == 'qwen3':
        language_model = base
    else:
        raise ValueError('Unsupported MLX model architecture')

    class AdapterModel(nn.Module):
        def __init__(self):
            super().__init__()
            self.base = base
            args = language_model.args
            self.down = mx.random.normal((args.hidden_size, 8)).astype(mx.float32) * 0.01
            self.up = mx.zeros((8, args.vocab_size), dtype=mx.float32)
            self.adapter_active = False

        def baseline(self, hidden):
            return (language_model.model.embed_tokens.as_linear(hidden)
                    if language_model.args.tie_word_embeddings
                    else language_model.lm_head(hidden))

        def hidden(self, inputs, cache=None, **kwargs):
            return language_model.model(inputs, cache=cache, **kwargs)

        def __call__(self, inputs, cache=None, **kwargs):
            hidden = self.hidden(inputs, cache=cache, **kwargs)
            baseline = self.baseline(hidden)
            if not self.adapter_active:
                return baseline
            return baseline.astype(mx.float32) + hidden.astype(mx.float32) @ self.down @ self.up

        def make_cache(self):
            return make_prompt_cache(self.base)

        @property
        def layers(self):
            return self.base.layers

    return AdapterModel()


def main():
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim
    from mlx_lm.generate import BatchGenerator, StopSequences, generate_step
    from mlx_lm.models.cache import can_trim_prompt_cache, trim_prompt_cache
    from mlx_lm.utils import load_model
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    init = json.loads(sys.argv[1])
    config = json.loads((Path(init['model_dir']) / 'config.json').read_text())
    base, config = load_model(Path(init['model_dir']), strict=True)
    quantization = config.get('quantization') or config.get('quantization_config')
    base.eval()
    base.freeze()
    mx.eval(base.parameters())

    model = make_adapter_model(base, config['model_type'])
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
        metadata = {'model_type': config['model_type'], 'adapter_kind': 'output_rank8'}
        if training_steps is not None:
            metadata['training_steps'] = str(training_steps)
        mx.save_safetensors(str(temporary), {'down': model.down, 'up': model.up}, metadata=metadata)
        os.replace(temporary, path)

    if not adapter_loaded:
        save_adapter(adapter)
    prefix_cache = PrefixCache(model, can_trim_prompt_cache, trim_prompt_cache)
    send({'ready': True, 'backend': 'mlx', 'precision': 'quantized' if quantization else 'BF16',
          'quantization': quantization, 'adapter_loaded': adapter_loaded,
          'adapter_created': not adapter_loaded, 'adapter_active': model.adapter_active,
          'adapter_training_steps': training_steps, 'memory': memory_stats(mx)})

    def generate_batch(requests):
        if not 1 <= len(requests) <= 4:
            raise ValueError('Generation batches must contain one to four requests')
        generator = BatchGenerator(model, prefill_batch_size=len(requests),
                                   completion_batch_size=len(requests), prefill_step_size=128)
        trackers = [TokenTracker(request) for request in requests]
        mx.reset_peak_memory()
        try:
            uids = generator.insert(
                [request['ids'] for request in requests],
                max_tokens=[request['max_tokens'] for request in requests],
                samplers=[request_sampler(request, make_sampler) for request in requests],
                logits_processors=[request_logits_processors(request, make_logits_processors)
                                   for request in requests],
                stop_sequences=[StopSequences([[token] for token in request['stop_ids']])
                                for request in requests])
            indices = {uid: index for index, uid in enumerate(uids)}
            finished = set()
            while len(finished) < len(requests):
                responses = generator.next_generated()
                if not responses:
                    raise RuntimeError('Batch generator ended before all requests completed')
                for response in responses:
                    tracker = trackers[indices[response.uid]]
                    if not tracker.accept(response.token):
                        finished.add(response.uid)
                        if tracker.finish_reason == 'thinking_budget':
                            generator.remove([response.uid])
                    elif response.finish_reason is not None:
                        finished.add(response.uid)
            return [dict(tracker.result(), memory=memory_stats(mx)) for tracker in trackers]
        finally:
            generator.close()

    def generate(request):
        mx.reset_peak_memory()
        processors = request_logits_processors(request, make_logits_processors)
        # Processors that see the prompt would observe only the uncached suffix.
        cache, prompt, reused = prefix_cache.prepare(
            request['ids'], allow_reuse=not processors or request.get('presence_exclude_prompt', False))
        if reused and processors:
            processors = request_logits_processors(request, make_logits_processors, len(prompt))
        tracker = TokenTracker(request)
        generator = generate_step(mx.array(prompt), model, max_tokens=request['max_tokens'],
                                  sampler=request_sampler(request, make_sampler),
                                  logits_processors=processors, prompt_cache=cache,
                                  prefill_step_size=128)
        try:
            for token, _ in generator:
                token = int(token.item()) if hasattr(token, 'item') else int(token)
                if not tracker.accept(token):
                    break
        except Exception:
            prefix_cache.invalidate()
            raise
        finally:
            generator.close()
        prefix_cache.record(request['ids'], tracker.tokens)
        return dict(tracker.result(), cached_prompt_tokens=reused, memory=memory_stats(mx),
                    sampling={'temperature': request['temperature'], 'top_p': request.get('top_p', 0.8),
                              'presence_penalty': request.get('presence_penalty'),
                              'presence_context_size': request.get('presence_context_size', 20),
                              'presence_exclude_prompt': request.get('presence_exclude_prompt', False)})

    for line in sys.stdin:
        try:
            request = json.loads(line)
            op = request['op']
            if op == 'generate_batch':
                send({'results': generate_batch(request['requests'])})
            elif op == 'generate':
                send(generate(request))
            elif op == 'train':
                targets = request['targets']
                sequence = request['ids'] + targets[:-1]
                hidden = model.hidden(mx.array([sequence]))[:, len(request['ids']) - 1:, :]
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
                if quantization:
                    raise ValueError('Cannot merge an adapter into a quantized base; keep the adapter separate')
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
