# rhai-analyzer

[![Crates.io](https://img.shields.io/crates/v/rhai-analyzer.svg)](https://crates.io/crates/rhai-analyzer)
[![Docs.rs](https://docs.rs/rhai-analyzer/badge.svg)](https://docs.rs/rhai-analyzer)
[![License](https://img.shields.io/crates/l/rhai-analyzer.svg)](https://crates.io/crates/rhai-analyzer)
[![Build Status](https://img.shields.io/github/actions/workflow/status/isSerge/rhai-analyzer/ci.yml?branch=main)](https://github.com/isSerge/rhai-analyzer/actions)

Static analysis utilities for [Rhai](https://rhai.rs/) scripts.

## Why this exists

`rhai-analyzer` provides a way to extract information from a compiled `rhai::AST` without executing it. This is useful for:
1.  **Dependency Tracking**: Identify which external variables or data paths a script depends on (e.g., `input.value`, `settings.mode`).
2.  **Schema Validation**: Ensure a script only accesses allowed variables.
3.  **Optimization**: Pre-calculate or pre-fetch data based on what the script is guaranteed (or likely) to access.

## Features

- **Variable Path Extraction**: Collects all unique, fully-qualified variable paths (e.g., `user.profile.name`) accessed in a script.
- **Local Variable Tracking**: Identifies variables defined within the script (via `let` or loop iterations) to distinguish them from external dependencies.
- **String Comparison Analysis**: Maps variable paths to the literal strings they are compared against (via `==` or `!=`).
- **Domain Agnostic**: The analyzer doesn't make assumptions about your data model—it just reports what it finds.

## Installation

### Add via Cargo

```bash
cargo add rhai-analyzer
```

### Manual Configuration

Add the following to your `Cargo.toml`:

```toml
[dependencies]
rhai-analyzer = "0.1.0"
rhai = "1.22.2"
```

## Usage

### 1. Analyzing a Script

```rust
use rhai::Engine;
use rhai_analyzer::analyze_ast;

fn main() {
    let engine = Engine::new();
    let script = r#"
        if request.amount > 100 && user.role == "admin" {
            let status = "approved";
            print(status);
        }
    "#;

    let ast = engine.compile(script).unwrap();
    let result = analyze_ast(&ast);

    // Variables accessed in the script
    assert!(result.accessed_variables.contains("request.amount"));
    assert!(result.accessed_variables.contains("user.role"));

    // Local variables defined in the script
    assert!(result.local_variables.contains("status"));

    // String comparisons tracked
    let comparisons = result.string_comparisons.get("user.role").unwrap();
    assert!(comparisons.contains("admin"));
}
```

### 2. Result Data Structure

The `analyze_ast` function returns a `ScriptAnalysisResult`:

```rust
pub struct ScriptAnalysisResult {
    /// All unique, fully-qualified variable paths accessed in the script.
    pub accessed_variables: HashSet<String>,

    /// Local variables defined within the script via `let` or loops.
    pub local_variables: HashSet<String>,

    /// Maps a variable path to the set of string literals it is compared against.
    pub string_comparisons: HashMap<String, HashSet<String>>,
}
```

## License

* MIT license (http://opensource.org/licenses/MIT)

