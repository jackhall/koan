# Foundation

> **Stale as work, kept as requirements.** Written against the old runtime,
> now behind `pending_rewrite`; the items below record what the language needs
> and are retired by the [rewrite](../rewrite/README.md) as it meets them.

Value-language primitives beneath the surface features — capabilities that
widen what a koan program can build, how its memory behaves, and what it can
say about its own effects. Each item here is a koan surface backed by substrate
behavior in the region-memory, scheduler, and type layers: values that close
reference cycles, producers that yield a sequence element by element, closures
that copy their captures instead of pinning the regions they came from, and
side effects expressed as a signature rather than a writer on the run frame.

## Items

Every requirements doc in this retired project.

- [Callable cells in copied containers](callable-cells-in-copied-containers.md)
- [Callable copy tuning](callable-copy-tuning.md)
- [Constructing circular values](circular-value-construction.md)
- [Definitions park on free names](definitions-park-on-free-names.md)
- [Destination-homed construction](destination-homed-construction.md)
- [A flattened dispatch registration pins its defining frame](flattened-registration-pins-its-frame.md)
- [A module's retention answer is conservative](module-retention-answer.md)
- [Module scope consolidation](module-scope-consolidation.md)
- [Monadic side effects](monadic-side-effects.md)
- [Yielding iterators](yielding-iterators.md)
