# Contributing to Ecphory

Ecphory is a new computing paradigm. Contributions are welcome, but please understand the project's philosophy before submitting.

## The Philosophy

1. **Intent is the atomic primitive.** Every design decision should be evaluated against this principle.
2. **The 18 Laws are discovered, not decreed.** If you find evidence that a law is wrong, that's a contribution. If you want to add a law, it must be discovered through implementation, not theorized.
3. **Rust is the bootstrap, not the destination.** The architecture is language-agnostic. Rust is Law 8: Legacy Acknowledged, Not Worshipped.
4. **Ethics is architecture, not a feature.** The immune system is not optional. Safety is not a tradeoff against capability.
5. **The human decides.** Law 18 is immutable.

## How to Contribute

### Found a bug?
Open an issue. Include: what you expected, what happened, and the minimal reproduction.

### Have a design idea?
Open an issue first. Describe the problem you're solving, not just the solution. Reference the relevant Laws.

### Want to implement something?
1. Check the roadmap in the architecture document
2. Open an issue describing what you want to build
3. Wait for discussion before starting (the paradigm has specific design constraints)
4. Submit a PR with tests that verify semantic properties (Law 9)

### Writing tests?
Tests verify semantic properties, not implementation details. "Identical contents produce identical signatures" is a good test. "The HashMap has 3 entries" is not.

## Code Standards

- `cargo test` must pass
- `cargo clippy` must be clean
- Zero external dependencies in core library (bootstrap deps in CLI are acceptable)
- Every design decision documented in comments with reasoning
- Flag open questions with `// DECISION:` for review

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
