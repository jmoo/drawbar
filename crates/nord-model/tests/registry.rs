//! The anti-rot suite: every string the Electro 5 model names is the registry's own.
//!
//! Rules are stringly by design — the registry is the vocabulary, and a parallel type
//! system would be a second place to rename a field. What makes that safe is that a
//! renamed field fails here, in the default suite, rather than in a GUI at runtime.

use nord_format::fields::FieldSpec;
use nord_format::formats::ne5;
use nord_format::layout::{BodyLayout, LayoutField};
use nord_model::{electro5::electro5, PathPattern, Rule};

fn specs() -> Vec<FieldSpec> {
    ne5::Program::field_specs()
}

fn program() -> ne5::Program {
    ne5::program::new((0, 0).try_into().expect("a program slot")).body
}

/// Every registered path the pattern names.
fn named(pattern: &PathPattern) -> Vec<String> {
    specs()
        .into_iter()
        .map(|spec| spec.name)
        .filter(|name| pattern.matches(name))
        .collect()
}

/// Every `(path, value)` pair the model names, from wherever a rule can name one.
fn pairs(rule: &Rule) -> Vec<(String, String)> {
    let owned = |pairs: Vec<(&str, &str)>| -> Vec<(String, String)> {
        pairs
            .into_iter()
            .map(|(p, v)| (p.to_string(), v.to_string()))
            .collect()
    };
    match rule {
        Rule::Gate { when, .. } => owned(when.pairs()),
        Rule::Narrow { path, when, to, .. } => {
            let mut out = owned(when.pairs());
            out.extend(to.iter().map(|value| (path.clone(), value.clone())));
            out
        }
        Rule::Couple { also, value, .. } => vec![(also.clone(), value.clone())],
    }
}

/// Every path a rule keys on resolves, and a pattern names at least one field.
///
/// A pattern matching nothing is the failure that matters: it is silent at runtime — the
/// gate simply never fires — and it is exactly what a renamed field leaves behind.
#[test]
fn every_path_the_model_names_is_in_the_registry() {
    let declared: Vec<String> = specs().into_iter().map(|spec| spec.name).collect();
    let resolves = |path: &str| {
        assert!(
            declared.iter().any(|name| name == path),
            "{path} is not a field of the Electro 5 program",
        );
    };

    for rule in electro5().rules() {
        match rule {
            Rule::Gate { controls, .. } => {
                for pattern in controls {
                    assert!(
                        !named(pattern).is_empty(),
                        "{} names no field of the Electro 5 program",
                        pattern.as_str(),
                    );
                }
            }
            Rule::Narrow { path, .. } => resolves(path),
            Rule::Couple { edit, also, .. } => {
                resolves(edit);
                resolves(also);
            }
        }
        for (path, _) in pairs(rule) {
            resolves(&path);
        }
    }
}

/// Every value spelling the model names is one the field itself takes.
///
/// Asked of `set_field` rather than of the legal list, so the check runs through the
/// same parse a `--set` does.
#[test]
fn every_value_the_model_names_is_one_the_field_takes() {
    for rule in electro5().rules() {
        for (path, value) in pairs(rule) {
            program()
                .set_field(&path, &value)
                .unwrap_or_else(|e| panic!("{path} = {value:?}: {e}"));
        }
    }
}

/// Every bit range a claimed field owns, over the program body.
fn claimed() -> Vec<(u32, u32)> {
    fn walk(fields: &[LayoutField], at: u32, out: &mut Vec<(u32, u32)>) {
        for field in fields {
            match field.nested {
                Some(nested) => walk(nested(), at + field.lo, out),
                None => out.push((at + field.lo, at + field.hi)),
            }
        }
    }
    let mut out = Vec::new();
    walk(<ne5::Program as BodyLayout>::layout(), 0, &mut out);
    out
}

/// A vestige names bits inside the body that no field claims.
///
/// ⚠️ If a field is ever declared over one of these, the vestige has to become a rule:
/// there is a path to key on then, and two descriptions of the same bits would drift.
#[test]
fn every_vestige_names_unclaimed_bits_of_the_body() {
    let claimed = claimed();
    let last = ne5::program::BODY_LEN as u32 * 8 - 1;

    for vestige in electro5().vestiges() {
        let (lo, hi) = vestige.bits;
        assert!(
            lo <= hi && hi <= last,
            "{} is outside the body",
            vestige.name
        );
        for (from, to) in &claimed {
            assert!(
                hi < *from || lo > *to,
                "{} overlaps a declared field at bits {from}..={to}",
                vestige.name,
            );
        }
    }
}
