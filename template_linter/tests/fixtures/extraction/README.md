Fixtures for extraction golden tests.

These are tiny, controlled `templatetags/*.py` modules used to ensure our
AST-derived extraction stays deterministic and "port-friendly" (Rust parity).

They intentionally avoid depending on Django runtime behavior beyond imports,
and are kept small so the expected extraction output is easy to review.
