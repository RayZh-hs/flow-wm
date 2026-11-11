//! Resolve the integer type of literals from declarations and callable signatures.
use super::ast::*;
mod expressions;
use self::expressions::field_type;
use std::collections::BTreeMap;
use vox_core::{
    diagnostics::{Diagnostic, DiagnosticBag},
    host::PackageManifest,
    integer::{IntegerType, IntegerValue},
    types::{RecordField, VoxType},
};

#[derive(Clone)]
struct Signature {
    names: Vec<String>,
    parameters: Vec<VoxType>,
    result: VoxType,
}
#[derive(Clone, Default)]
struct Context {
    values: BTreeMap<String, VoxType>,
    functions: BTreeMap<String, Signature>,
    result: Option<VoxType>,
    inferred_return: Option<VoxType>,
    structs: BTreeMap<String, Vec<RecordField>>,
}
fn unknown() -> VoxType {
    VoxType::opaque_surface("Unknown")
}

pub fn syntax_type(ty: &TypeSyntax) -> VoxType {
    match &ty.kind {
        TypeKind::Named { name, arguments } => {
            let name = name.to_source_string();
            if let Some(integer) = IntegerType::from_name(&name) {
                return VoxType::integer(integer);
            }
            match name.as_str() {
                "Float" => VoxType::Float,
                "Bool" => VoxType::Bool,
                "String" => VoxType::String,
                "List" if arguments.len() == 1 => {
                    VoxType::List(Box::new(syntax_type(&arguments[0])))
                }
                _ => VoxType::opaque_surface(ty.to_source_string()),
            }
        }
        TypeKind::Nullable(inner) => VoxType::Nullable(Box::new(syntax_type(inner))),
        TypeKind::Grouped(inner) => syntax_type(inner),
        TypeKind::Tuple(items) => VoxType::Tuple(items.iter().map(syntax_type).collect()),
        TypeKind::Record(fields) => VoxType::Record(
            fields
                .iter()
                .map(|f| RecordField {
                    name: f.name.clone(),
                    ty: syntax_type(&f.ty),
                })
                .collect(),
        ),
        _ => VoxType::opaque_surface(ty.to_source_string()),
    }
}

pub fn normalize(unit: &mut FrontendUnit, manifests: &[PackageManifest]) -> DiagnosticBag {
    let mut context = Context::default();
    for package in manifests {
        for ty in &package.types {
            context.structs.insert(
                format!("{}.{}", package.package.as_str(), ty.name.name),
                ty.fields
                    .iter()
                    .map(|f| RecordField {
                        name: f.name.clone(),
                        ty: f.ty.clone(),
                    })
                    .collect(),
            );
        }
        for f in &package.functions {
            context.functions.insert(
                format!("{}.{}", package.package.as_str(), f.name),
                Signature {
                    names: f.parameters.iter().map(|p| p.name.clone()).collect(),
                    parameters: f.parameters.iter().map(|p| p.ty.clone()).collect(),
                    result: f.return_type.clone(),
                },
            );
        }
    }
    for item in &unit.syntax.items {
        match item {
            TopLevelItem::Function(f) => {
                context.functions.insert(f.name.clone(), signature(f));
            }
            TopLevelItem::Struct(s) => {
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|f| RecordField {
                        name: f.name.clone(),
                        ty: syntax_type(&f.ty),
                    })
                    .collect();
                context.functions.insert(
                    s.name.clone(),
                    Signature {
                        names: fields.iter().map(|f| f.name.clone()).collect(),
                        parameters: fields.iter().map(|f| f.ty.clone()).collect(),
                        result: VoxType::opaque_surface(s.name.clone()),
                    },
                );
                context.structs.insert(s.name.clone(), fields);
                for method in &s.methods {
                    context
                        .functions
                        .insert(format!("{}.{}", s.name, method.name), signature(method));
                }
            }

            TopLevelItem::Value(v) if v.ty.is_some() => {
                context
                    .values
                    .insert(v.name.clone(), syntax_type(v.ty.as_ref().unwrap()));
            }
            TopLevelItem::Param(p) => {
                context.values.insert(p.name.clone(), syntax_type(&p.ty));
            }
            TopLevelItem::Import(import) => {
                for import in import.expanded() {
                    let module = import.module.to_source_string();
                    if let Some(items) = &import.items {
                        for item in items {
                            if let Some(sig) = context
                                .functions
                                .get(&format!("{module}.{}", item.name))
                                .cloned()
                            {
                                context.functions.insert(
                                    item.alias.clone().unwrap_or_else(|| item.name.clone()),
                                    sig,
                                );
                            }
                        }
                    } else {
                        let alias = import.alias.clone().unwrap_or_else(|| {
                            import.module.segments.last().cloned().unwrap_or_default()
                        });
                        let functions: Vec<_> = context
                            .functions
                            .iter()
                            .filter_map(|(name, sig)| {
                                name.strip_prefix(&format!("{module}."))
                                    .map(|rest| (format!("{alias}.{rest}"), sig.clone()))
                            })
                            .collect();
                        context.functions.extend(functions);
                    }
                }
            }
            _ => {}
        }
    }
    let mut walker = Walker {
        errors: DiagnosticBag::default(),
    };
    // Resolve inferred local function results before using them as literal contexts.
    for _ in 0..unit.syntax.items.len().max(1) {
        let mut changed = false;
        for item in &mut unit.syntax.items {
            if let TopLevelItem::Function(f) = item {
                let result = walker.function(f, &context);
                if result != unknown() {
                    if let Some(signature) = context.functions.get_mut(&f.name) {
                        if signature.result != result {
                            signature.result = result;
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    for item in &mut unit.syntax.items {
        match item {
            TopLevelItem::Value(v) => {
                let expected = v.ty.as_ref().map(syntax_type);
                let ty = walker.expr(&mut v.initializer, expected.as_ref(), &mut context);
                context
                    .values
                    .insert(v.name.clone(), expected.unwrap_or(ty));
            }
            TopLevelItem::Param(p) => {
                if let Some(default) = &mut p.default {
                    walker.expr(default, Some(&syntax_type(&p.ty)), &mut context);
                }
            }
            TopLevelItem::Function(f) => {
                walker.function(f, &context);
            }
            TopLevelItem::Struct(s) => {
                for f in &mut s.methods {
                    walker.function(f, &context);
                }
            }
            TopLevelItem::Trait(t) => {
                for f in &mut t.methods {
                    if let Some(body) = &mut f.body {
                        let mut scope = context.clone();
                        scope.result = f.return_type.as_ref().map(syntax_type);
                        scope.inferred_return = None;
                        for p in &f.parameters {
                            scope.values.insert(p.name.clone(), syntax_type(&p.ty));
                        }
                        let result = scope.result.clone();
                        walker.expr(body, result.as_ref(), &mut scope);
                    }
                }
            }
            TopLevelItem::Impl(i) => {
                for f in &mut i.methods {
                    walker.function(f, &context);
                }
            }
            TopLevelItem::Statement(s) => walker.item(s, &mut context),
            _ => {}
        }
    }
    if let Some(result) = &mut unit.syntax.result {
        walker.expr(result, None, &mut context);
    }
    unit.parameters = FrontendUnit::from_syntax(unit.syntax.clone()).parameters;
    walker.errors
}

fn signature(f: &FunctionDecl) -> Signature {
    Signature {
        names: f.parameters.iter().map(|p| p.name.clone()).collect(),
        parameters: f.parameters.iter().map(|p| syntax_type(&p.ty)).collect(),
        result: f
            .return_type
            .as_ref()
            .map(syntax_type)
            .unwrap_or_else(unknown),
    }
}
struct Walker {
    errors: DiagnosticBag,
}
impl Walker {
    fn function(&mut self, f: &mut FunctionDecl, parent: &Context) -> VoxType {
        let mut scope = parent.clone();
        scope.result = f.return_type.as_ref().map(syntax_type);
        for p in &mut f.parameters {
            let ty = syntax_type(&p.ty);
            if let Some(value) = &mut p.default {
                self.expr(value, Some(&ty), &mut scope);
            }
            scope.values.insert(p.name.clone(), ty);
        }
        let result = scope.result.clone();
        let actual = self.expr(&mut f.body, result.as_ref(), &mut scope);
        result.unwrap_or(actual)
    }
    fn item(&mut self, item: &mut BlockItem, scope: &mut Context) {
        match item {
            BlockItem::LocalValue(v) => {
                let expected = v.ty.as_ref().map(syntax_type);
                let ty = self.expr(&mut v.initializer, expected.as_ref(), scope);
                scope.values.insert(v.name.clone(), expected.unwrap_or(ty));
            }
            BlockItem::Assignment(a) => {
                let ty = if let Some(target) = &mut a.target {
                    let ty = self.expr(target, None, scope);
                    field_type(&ty, &a.name, scope)
                } else {
                    scope.values.get(&a.name).cloned()
                };
                self.expr(&mut a.value, ty.as_ref(), scope);
            }
            BlockItem::CompoundAssignment(a) => {
                let ty = if let Some(target) = &mut a.target {
                    let ty = self.expr(target, None, scope);
                    field_type(&ty, &a.name, scope)
                } else {
                    scope.values.get(&a.name).cloned()
                };
                self.expr(&mut a.value, ty.as_ref(), scope);
            }
            BlockItem::Return(r) => {
                let ty = scope.result.clone();
                if let Some(value) = &mut r.value {
                    let inferred = self.expr(value, ty.as_ref(), scope);
                    if scope.result.is_none() {
                        scope.inferred_return = Some(inferred);
                    }
                }
            }
            BlockItem::Expr(e) | BlockItem::BlockStatement(e) => {
                self.expr(e, None, scope);
            }
            BlockItem::Panic(p) => {
                for part in &mut p.message.parts {
                    if let StringPart::Interpolation(e) = part {
                        self.expr(e, None, scope);
                    }
                }
            }
            _ => {}
        }
    }
    fn block(
        &mut self,
        block: &mut BlockExpr,
        expected: Option<&VoxType>,
        parent: &Context,
    ) -> VoxType {
        let mut scope = parent.clone();
        for item in &mut block.items {
            self.item(item, &mut scope);
        }
        block
            .trailing
            .as_mut()
            .map(|e| self.expr(e, expected, &mut scope))
            .unwrap_or_else(|| {
                scope
                    .inferred_return
                    .clone()
                    .unwrap_or_else(|| VoxType::Tuple(vec![]))
            })
    }
}
