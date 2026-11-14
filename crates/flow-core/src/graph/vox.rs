//! Conservative, non-executing schema inference using Vox's own parsed syntax.
use super::{Pin, Type};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use vox_compiler::frontend::{FrontendUnit, analyze_source, ast::*};
use vox_core::source::SourceText;

type Scope = BTreeMap<String, Type>;
type SyntaxCache = BTreeMap<String, Option<Rc<FrontendUnit>>>;
thread_local! { static SYNTAX: RefCell<SyntaxCache> = const { RefCell::new(BTreeMap::new()) }; }

pub fn vox_syntax_error(source: &str) -> Option<String> {
    analyze_source(&SourceText::new(
        "flowwm/node.vox",
        1,
        format!("script flowwm.node;\n{source}"),
    ))
    .err()
    .map(|error| error.to_string())
}

pub fn vox_outputs(source: &str, inputs: &[Pin]) -> Option<Scope> {
    let unit = SYNTAX.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 128 && !cache.contains_key(source) {
            cache.clear();
        }
        cache
            .entry(source.to_owned())
            .or_insert_with(|| {
                analyze_source(&SourceText::new(
                    "flowwm/node.vox",
                    1,
                    format!("script flowwm.node;\n{source}"),
                ))
                .ok()
                .map(Rc::new)
            })
            .clone()
    })?;
    let mut scope = Scope::from([
        (
            "inputs".into(),
            Type::Record {
                fields: inputs
                    .iter()
                    .filter(|pin| !pin.ty.is_signal())
                    .map(|pin| (pin.name.clone(), pin.ty.clone()))
                    .collect(),
                open: false,
            },
        ),
        (
            "args".into(),
            inputs
                .iter()
                .find_map(|pin| match &pin.ty {
                    Type::Signal { args, open } => Some(Type::Record {
                        fields: args.clone(),
                        open: *open,
                    }),
                    _ => None,
                })
                .unwrap_or_else(Type::record),
        ),
    ]);
    for item in &unit.syntax.items {
        if let TopLevelItem::Value(value) = item {
            scope.insert(
                value.name.clone(),
                annotated(&value.ty, &value.initializer, &scope),
            );
        }
    }
    match infer(unit.syntax.result.as_ref()?, &scope) {
        Type::Record { fields, .. } => Some(fields),
        _ => None,
    }
}

fn annotated(ty: &Option<TypeSyntax>, expr: &Expr, scope: &Scope) -> Type {
    ty.as_ref()
        .map(syntax_type)
        .unwrap_or_else(|| infer(expr, scope))
}
fn syntax_type(ty: &TypeSyntax) -> Type {
    match &ty.kind {
        TypeKind::Named { name, arguments } => match name.to_source_string().as_str() {
            "Bool" => Type::Bool,
            "Int" | "Int64" => Type::Int,
            "UInt" | "UInt64" => Type::UInt,
            "UInt8" => Type::UInt8,
            "Float" => Type::Float,
            "String" => Type::String,
            "List" => Type::List {
                item: Box::new(arguments.first().map(syntax_type).unwrap_or(Type::Any)),
            },
            _ => Type::Any,
        },
        TypeKind::Record(fields) => Type::Record {
            fields: fields
                .iter()
                .map(|f| (f.name.clone(), syntax_type(&f.ty)))
                .collect(),
            open: false,
        },
        TypeKind::Grouped(inner) => syntax_type(inner),
        _ => Type::Any,
    }
}
fn block(block: &BlockExpr, scope: &Scope) -> Type {
    let mut scope = scope.clone();
    for item in &block.items {
        match item {
            BlockItem::LocalValue(value) => {
                scope.insert(
                    value.name.clone(),
                    annotated(&value.ty, &value.initializer, &scope),
                );
            }
            // Mutable assignments need control-flow analysis; keep their type unknown.
            BlockItem::Assignment(_) | BlockItem::CompoundAssignment(_) => {
                return Type::Any;
            }
            _ => {}
        }
    }
    block
        .trailing
        .as_ref()
        .map(|expr| infer(expr, &scope))
        .unwrap_or(Type::Any)
}
fn infer(expr: &Expr, scope: &Scope) -> Type {
    match &expr.kind {
        ExprKind::Integer(_) => Type::Int,
        ExprKind::UInt(_) => Type::UInt,
        ExprKind::FixedInteger(value) => match value.ty().name() {
            "UInt8" => Type::UInt8,
            "UInt64" => Type::UInt,
            "Int64" => Type::Int,
            _ => Type::Any,
        },
        ExprKind::Float(_) => Type::Float,
        ExprKind::Bool(_) => Type::Bool,
        ExprKind::String(_) => Type::String,
        ExprKind::Record(fields) => Type::Record {
            fields: fields
                .iter()
                .map(|field| {
                    (
                        field.name.clone(),
                        annotated(&field.ty, &field.value, scope),
                    )
                })
                .collect(),
            open: false,
        },
        ExprKind::Name(name) => scope
            .get(&name.to_source_string())
            .cloned()
            .unwrap_or(Type::Any),
        ExprKind::Field { target, name } | ExprKind::SafeField { target, name } => {
            match infer(target, scope) {
                Type::Record { fields, .. } => fields.get(name).cloned().unwrap_or(Type::Any),
                _ => Type::Any,
            }
        }
        ExprKind::Index { target, .. } => match infer(target, scope) {
            Type::List { item } => *item,
            _ => Type::Any,
        },
        ExprKind::List(items) => {
            let first = items
                .first()
                .map(|item| infer(item, scope))
                .unwrap_or(Type::Any);
            Type::List {
                item: Box::new(if items.iter().all(|item| infer(item, scope) == first) {
                    first
                } else {
                    Type::Any
                }),
            }
        }
        ExprKind::Unary {
            op: UnaryOp::Not, ..
        } => Type::Bool,
        ExprKind::Unary { expr, .. } | ExprKind::NonNull { target: expr } => infer(expr, scope),
        ExprKind::Binary { left, op, right } => {
            if matches!(
                op,
                BinaryOp::Less
                    | BinaryOp::LessEqual
                    | BinaryOp::Greater
                    | BinaryOp::GreaterEqual
                    | BinaryOp::Equal
                    | BinaryOp::NotEqual
                    | BinaryOp::And
                    | BinaryOp::Or
            ) {
                return Type::Bool;
            }
            let a = infer(left, scope);
            let b = infer(right, scope);
            if a == b {
                a
            } else if matches!(
                (&a, &b),
                (Type::Float, Type::Int | Type::UInt) | (Type::Int | Type::UInt, Type::Float)
            ) {
                Type::Float
            } else {
                Type::Any
            }
        }
        ExprKind::Block(body) => block(body, scope),
        ExprKind::If(value) => {
            let Some(other) = &value.else_branch else {
                return Type::Any;
            };
            let ty = block(other, scope);
            if value
                .branches
                .iter()
                .all(|branch| block(&branch.body, scope) == ty)
            {
                ty
            } else {
                Type::Any
            }
        }
        _ => Type::Any,
    }
}
