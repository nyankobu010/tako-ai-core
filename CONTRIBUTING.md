# Contributing to tako

Thanks for your interest in `tako`. This page covers the dev setup and the
workflow for contributing.

## Dev environment

Prerequisites:

- Rust **1.82** or newer (`rustup default stable`)
- Python **3.10** or newer
- [`uv`](https://docs.astral.sh/uv/) (recommended) or `pip` + `venv`
- [`maturin`](https://www.maturin.rs/) (`uv tool install maturin` or `pipx install maturin`)
- A C linker (clang on macOS, MSVC build tools on Windows, gcc/clang on Linux)

Set up:

```bash
git clone https://github.com/nyankobu010/tako-ai-core
cd tako-ai-core

# Rust
cargo build --workspace
cargo test --workspace

# Python (mixed Rust/Python project; maturin builds the extension)
uv venv .venv
source .venv/bin/activate                 # Windows: .venv\Scripts\activate
uv pip install -e ".[dev,docs]"
maturin develop --release
pytest -q
```

`.env.example` lists the env vars that examples and live-API tests look for.
Copy it to `.env` and fill in your own keys; `.env` is gitignored.

## Workflow

- Open an issue first for non-trivial changes.
- Use [Conventional Commits](https://www.conventionalcommits.org/): `feat(tako-core): add Foo trait`, `fix(tako-py): release GIL before block_on`, `docs: update quickstart`.
- All commits must be DCO-signed-off (`git commit -s`).
- One logical change per commit; one logical concern per PR.
- All PRs trigger CI (fmt, clippy, cargo test, pytest, audits) on Linux/macOS/Windows.

## Code style

Rust:

```bash
cargo fmt --all
cargo clippy --workspace --all-features -- -D warnings
```

Python:

```bash
ruff format python/ tests/python/
ruff check python/ tests/python/
mypy python/tako
```

## Tests

- Rust: `cargo test --workspace --all-features`. Per-crate: `cargo test -p tako-core`.
- Python: `pytest -q`. The Python suite uses an in-process `FakeProvider`; no API keys needed.
- Integration: end-to-end tests under `tests/rust/` use `wiremock` against canned vendor responses.

## Docs

Docs are built with `mkdocs-material`:

```bash
uv pip install -e ".[docs]"
mkdocs serve -f docs/mkdocs.yml
```

## Good first issues

Filter issues with the [`good first issue`](https://github.com/nyankobu010/tako-ai-core/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)
label.

## Releasing

Releases are tag-driven. Tag `v0.X.Y` on `main`, push the tag, and the
`wheels.yml` workflow builds and publishes signed artefacts to PyPI.
