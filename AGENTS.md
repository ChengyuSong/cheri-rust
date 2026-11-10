# Agent instructions

## Fork

This repository is Chengyu Song's CHERI research prototype (`ChengyuSong/cheri-rust`), not `rust-lang/rust`. It targets riscv64 purecap (CHERI-Toooba). Changes stay on this fork. Do not push to `rust-lang/rust` or any other upstream remote, and do not open an upstream pull request.

## LLM contributions

LLM-written code, tests, commit messages, and pull request text are allowed here.

- Every commit subject ends with `[LLM-generated]`.
- The pull request body says the change was LLM-generated.
- Do not merge unless a human explicitly asks.

## Upstream policy (does not apply)

The [Rust project LLM usage policy](https://forge.rust-lang.org/policies/llm-usage.html) governs contributions to `rust-lang/rust`. It does not apply to this fork. Nothing in this tree is upstreamed. The upstream agent gates (named reviewer before any edit, prohibited pull-request and comment text, the soundness implementation ban, and the personal-use exemption) are not in force here. Follow the fork rules above.

## Repository guidance

This checkout is the CHERI research fork of the Rust compiler sources.
Start with [`CONTRIBUTING.md`](CONTRIBUTING.md) and the [dev-guide's instructions for LLMs][llm-writing], then route specialized work as follows:

[llm-writing]: https://rustc-dev-guide.rust-lang.org/llm-guidance/writing.html

- Standard library: [std-dev-guide]
- Compiler: [rustc-dev-guide]
- Build or run rustc: [building and running rustc]
- Tests: [running tests], [adding tests], and [compiletest directives]
- Formatting or tidy: [formatting and tidy]
- Architecture or layout: [compiler architecture] and [repository layout]
- Subtrees, submodules, or tools: [external repositories]
- Pull requests and review: [contribution process]

[rustc-dev-guide]: src/doc/rustc-dev-guide/
[std-dev-guide]: https://std-dev-guide.rust-lang.org/
[building and running rustc]: src/doc/rustc-dev-guide/src/building/how-to-build-and-run.md
[running tests]: src/doc/rustc-dev-guide/src/tests/running.md
[adding tests]: src/doc/rustc-dev-guide/src/tests/adding.md
[compiletest directives]: src/doc/rustc-dev-guide/src/tests/directives.md
[formatting and tidy]: src/doc/rustc-dev-guide/src/conventions.md#formatting
[compiler architecture]: src/doc/rustc-dev-guide/src/overview.md
[repository layout]: src/doc/rustc-dev-guide/src/compiler-src.md
[contribution process]: src/doc/rustc-dev-guide/src/contributing.md

[`x.py` is the build tool for this repository][building and running rustc].
Invoke it as `./x`, the default entry point for builds, tests, and formatting.
Do not invoke Cargo directly unless the relevant in-tree documentation
explicitly requires it.

For Rust formatting, use `./x fmt`; do not invoke `rustfmt` directly.
