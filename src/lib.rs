//! Static analysis utilities for Rhai scripts.
//!
//! Traverses a compiled [`rhai::AST`] to extract variable access paths,
//! local variable definitions, and string literal comparisons. The output is
//! entirely domain-agnostic — callers decide what to do with the results.

use std::collections::{HashMap, HashSet};

use rhai::{AST, ASTNode, Expr, Stmt};

/// The result of a static analysis pass over a Rhai [`AST`].
#[derive(Debug, Default)]
pub struct ScriptAnalysisResult {
    /// All unique, fully-qualified variable paths accessed in the script
    /// (e.g. `"tx.value"`, `"log.name"`).
    pub accessed_variables: HashSet<String>,

    /// Local variables defined within the script via `let` or loop iteration
    /// variables.
    pub local_variables: HashSet<String>,

    /// Maps a fully-qualified variable path to the set of string literals it
    /// is compared against (via `==` or `!=`) anywhere in the script.
    ///
    /// Example: `log.name == "Transfer"` produces
    /// `"log.name" => {"Transfer"}`.
    pub string_comparisons: HashMap<String, HashSet<String>>,
}

/// Traverses a compiled [`AST`] and returns a [`ScriptAnalysisResult`].
///
/// This is the primary entry point for the analyzer.
pub fn analyze_ast(ast: &AST) -> ScriptAnalysisResult {
    let mut result = ScriptAnalysisResult::default();

    ast.walk(&mut |nodes: &[ASTNode]| {
        let parent = nodes.len().checked_sub(2).map(|i| &nodes[i]);

        match nodes.last().unwrap() {
            // ---------------------------------------------------------------
            // Statement nodes
            // ---------------------------------------------------------------
            ASTNode::Stmt(stmt) => match *stmt {
                // Track locally-defined variables.
                Stmt::Var(var_def, _, _) => {
                    result.local_variables.insert(var_def.0.name.to_string());
                }
                Stmt::For(for_loop, _) => {
                    result.local_variables.insert(for_loop.0.name.to_string());
                    if let Some(counter) = &for_loop.1 {
                        result.local_variables.insert(counter.name.to_string());
                    }
                }
                // Rhai compiles a bare `a == "b"` as Stmt::FnCall (not
                // Stmt::Expr), so the FnCall is never surfaced as
                // ASTNode::Expr. Check for comparisons here.
                Stmt::FnCall(fn_call, _)
                    if fn_call.namespace.is_empty() && fn_call.args.len() == 2 =>
                {
                    match fn_call.name.as_str() {
                        "==" | "!=" => {
                            record_string_comparison(
                                &fn_call.args[0],
                                &fn_call.args[1],
                                &mut result,
                            );
                            record_string_comparison(
                                &fn_call.args[1],
                                &fn_call.args[0],
                                &mut result,
                            );
                        }
                        _ => {}
                    }
                }
                _ => {}
            },

            // ---------------------------------------------------------------
            // Expression nodes
            // ---------------------------------------------------------------
            ASTNode::Expr(expr) => {
                // String comparison detection (single node; walker recurses
                // for us into FnCall args, And, Or, etc.).
                if let Expr::FnCall(fn_call, _) = *expr
                    && fn_call.namespace.is_empty()
                    && fn_call.args.len() == 2
                {
                    match fn_call.name.as_str() {
                        "==" | "!=" => {
                            record_string_comparison(
                                &fn_call.args[0],
                                &fn_call.args[1],
                                &mut result,
                            );
                            record_string_comparison(
                                &fn_call.args[1],
                                &fn_call.args[0],
                                &mut result,
                            );
                        }
                        _ => {}
                    }
                }

                // Variable path tracking.
                // Skip Property nodes — they are only the rhs component of a
                // Dot expression and are covered when the parent Dot node is
                // visited.  Inserting them standalone would wrongly emit bare
                // field names from chains like `arr[0].name`.
                if !matches!(*expr, Expr::Property(..))
                    && let Some(path) = get_full_variable_path(expr)
                {
                    if !parent_subsumes(parent) {
                        result.accessed_variables.insert(path);
                    }
                    // Index expressions also expose their index operand as
                    // a separate access.
                    if let Expr::Index(bin, _, _) = *expr
                        && let Some(idx_path) = get_full_variable_path(&bin.rhs)
                    {
                        result.accessed_variables.insert(idx_path);
                    }
                }
            }

            _ => {}
        }

        true
    });

    result
}

// ---------------------------------------------------------------------------
// Parent-subsumption check
// ---------------------------------------------------------------------------

/// Returns `true` when the current expression's variable path is already
/// covered by its parent, so we should not insert it separately.
///
/// * A [`Expr::Dot`] parent that forms a complete path will have inserted the
///   longer dotted path (e.g. `"tx.value"`) itself, so we skip the child
///   `Variable("tx")`.
/// * An [`Expr::Index`] parent handles both the lhs path and the rhs index
///   path explicitly, so we skip both children.
fn parent_subsumes(parent: Option<&ASTNode>) -> bool {
    match parent {
        Some(ASTNode::Expr(parent_expr)) => match *parent_expr {
            Expr::Dot(..) => get_full_variable_path(parent_expr).is_some(),
            Expr::Index(..) => true,
            _ => false,
        },
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Path reconstruction
// ---------------------------------------------------------------------------

/// Attempts to reconstruct a full dotted variable path (e.g. `"tx.value"`)
/// from an expression.
fn get_full_variable_path(expr: &Expr) -> Option<String> {
    fn collect_path(expr: &Expr, parts: &mut Vec<String>) -> bool {
        match expr {
            Expr::Dot(binary_expr, _, _) => {
                collect_path(&binary_expr.lhs, parts) && collect_path(&binary_expr.rhs, parts)
            }
            Expr::Property(prop_info, _) => {
                parts.push(prop_info.2.to_string());
                true
            }
            Expr::Variable(var_info, _, _) => {
                parts.push(var_info.1.to_string());
                true
            }
            Expr::Index(binary_expr, _, _) => collect_path(&binary_expr.lhs, parts),
            _ => false,
        }
    }

    let mut path_parts = Vec::new();
    if collect_path(expr, &mut path_parts) && !path_parts.is_empty() {
        Some(path_parts.join("."))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// String comparison tracking
// ---------------------------------------------------------------------------

/// If `lhs` is a variable path and `rhs` is a string literal, records the
/// comparison in `result.string_comparisons`.
fn record_string_comparison(lhs: &Expr, rhs: &Expr, result: &mut ScriptAnalysisResult) {
    if let Some(var_path) = get_full_variable_path(lhs)
        && let Expr::StringConstant(string_val, _) = rhs
    {
        result
            .string_comparisons
            .entry(var_path)
            .or_default()
            .insert(string_val.to_string());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rhai::{Engine, ParseError};

    use super::*;

    fn analyze_script(script: &str) -> Result<ScriptAnalysisResult, ParseError> {
        let engine = Engine::new();
        let ast = engine.compile(script)?;
        Ok(analyze_ast(&ast))
    }

    #[test]
    fn test_simple_binary_op() {
        let result = analyze_script("tx.value > 100").unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["tx.value".to_string()])
        );
    }

    #[test]
    fn test_logical_operators() {
        let script = r#"tx.from == owner && log.name != "Transfer" || block.number > 1000"#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "tx.from".to_string(),
                "owner".to_string(),
                "log.name".to_string(),
                "block.number".to_string(),
            ])
        );
    }

    #[test]
    fn test_multiple_variables_and_coalesce() {
        let result = analyze_script("tx.from ?? fallback_addr.address").unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["tx.from".to_string(), "fallback_addr.address".to_string()])
        );
    }

    #[test]
    fn test_deeply_nested_variable() {
        let script = r#"log.params.level_one.level_two.user == "admin""#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["log.params.level_one.level_two.user".to_string()])
        );
    }

    #[test]
    fn test_variables_in_function_calls() {
        let result = analyze_script("my_func(tx.value, log.params.user, 42)").unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["tx.value".to_string(), "log.params.user".to_string()])
        );
    }

    #[test]
    fn test_variables_in_let_and_if() {
        let script = r#"
            let threshold = config.min_value;
            if tx.value > threshold && tx.to != blacklist.address {
                true
            } else {
                false
            }
        "#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "config.min_value".to_string(),
                "tx.value".to_string(),
                "threshold".to_string(),
                "tx.to".to_string(),
                "blacklist.address".to_string()
            ])
        );
    }

    #[test]
    fn test_variables_in_loops() {
        let script = r#"
            for item in tx.items {
                if item.cost > max_cost {
                    return false;
                }
            }
            while x < limit {
                x = x + 1;
            }
        "#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "tx.items".to_string(),
                "item.cost".to_string(),
                "max_cost".to_string(),
                "x".to_string(),
                "limit".to_string(),
            ])
        );
    }

    #[test]
    fn test_variables_in_strings_or_comments_are_ignored() {
        let script = r#"
            // This is a comment about tx.value
            let x = "this string mentions log.name";
            tx.from == "0x123"
        "#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["tx.from".to_string()])
        );
    }

    #[test]
    fn test_indexing_expression() {
        let script = r#"tx.logs[0].name == "Transfer" && some_array[tx.index] > 100"#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "tx.logs".to_string(),
                "some_array".to_string(),
                "tx.index".to_string(),
            ])
        );
    }

    #[test]
    fn test_method_calls() {
        let script = r#"my_array.contains(tx.value) && other_var.to_string() == "hello""#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "my_array".to_string(),
                "tx.value".to_string(),
                "other_var".to_string(),
            ])
        );
    }

    #[test]
    fn test_switch_statement() {
        let script = r#"
            switch tx.action {
                "transfer" => do_transfer(log.params.amount),
                "approve" if log.approved => do_approve(),
                _ => do_nothing(contract.address)
            }
        "#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "tx.action".to_string(),
                "log.params.amount".to_string(),
                "log.approved".to_string(),
                "contract.address".to_string(),
            ])
        );
    }

    #[test]
    fn test_no_variables() {
        let result = analyze_script("1 + 1 == 2").unwrap();
        assert!(result.accessed_variables.is_empty());
    }

    #[test]
    fn test_array_and_map_literals() {
        let script = r#"
            let my_array = [tx.value, log.topic];
            let my_map = #{ a: some.value, b: 42 };
            my_array[0] > my_map.a
        "#;
        let result = analyze_script(script).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from([
                "tx.value".to_string(),
                "log.topic".to_string(),
                "some.value".to_string(),
                "my_array".to_string(),
                "my_map.a".to_string(),
            ])
        );
    }

    #[test]
    fn test_string_comparison_simple() {
        let result = analyze_script(r#"log.name == "Transfer""#).unwrap();
        assert_eq!(
            result.accessed_variables,
            HashSet::from(["log.name".to_string()])
        );
        let names = result.string_comparisons.get("log.name").unwrap();
        assert_eq!(names, &HashSet::from(["Transfer".to_string()]));
    }

    #[test]
    fn test_string_comparison_reversed() {
        let result = analyze_script(r#""Approval" == log.name"#).unwrap();
        let names = result.string_comparisons.get("log.name").unwrap();
        assert_eq!(names, &HashSet::from(["Approval".to_string()]));
    }

    #[test]
    fn test_string_comparison_in_logical_or() {
        let result = analyze_script(r#"tx.value > 100 || log.name == "Deposit""#).unwrap();
        let names = result.string_comparisons.get("log.name").unwrap();
        assert_eq!(names, &HashSet::from(["Deposit".to_string()]));
    }

    #[test]
    fn test_string_comparison_multiple_values() {
        let result = analyze_script(r#"log.name == "Transfer" || log.name == "Approval""#).unwrap();
        let names = result.string_comparisons.get("log.name").unwrap();
        assert_eq!(
            names,
            &HashSet::from(["Transfer".to_string(), "Approval".to_string()])
        );
    }

    #[test]
    fn test_string_comparison_inequality() {
        let result = analyze_script(r#"log.name != "Transfer""#).unwrap();
        let names = result.string_comparisons.get("log.name").unwrap();
        assert_eq!(names, &HashSet::from(["Transfer".to_string()]));
    }

    #[test]
    fn test_string_comparison_different_path() {
        let result = analyze_script(r#"tx.status == "success" && tx.type != "mint""#).unwrap();
        let statuses = result.string_comparisons.get("tx.status").unwrap();
        assert_eq!(statuses, &HashSet::from(["success".to_string()]));
        let types = result.string_comparisons.get("tx.type").unwrap();
        assert_eq!(types, &HashSet::from(["mint".to_string()]));
    }
}
