use super::*;

impl Walker {
    pub(super) fn expr(
        &mut self,
        expr: &mut Expr,
        expected: Option<&VoxType>,
        scope: &mut Context,
    ) -> VoxType {
        let actual = self.expr_inner(expr, expected, scope);
        let destination = expected.and_then(|ty| match ty {
            VoxType::Nullable(inner) => inner.integer_type(),
            _ => ty.integer_type(),
        });
        if let Some((from, to)) = actual.integer_type().zip(destination) {
            if from != to && from.can_widen_to(to) {
                expr.kind = ExprKind::IntegerWiden {
                    target: Box::new(expr.clone()),
                    ty: to,
                };
                return VoxType::integer(to);
            }
        }
        actual
    }
    fn expr_inner(
        &mut self,
        expr: &mut Expr,
        expected: Option<&VoxType>,
        scope: &mut Context,
    ) -> VoxType {
        let hint = expected.and_then(|ty| match ty {
            VoxType::Nullable(inner) => inner.integer_type(),
            _ => ty.integer_type(),
        });
        // A negative integer literal includes the signed minimum, whose positive
        // magnitude is not representable by the destination signed type.
        let literal = match &expr.kind {
            ExprKind::Integer(raw) => Some((raw.clone(), false)),
            ExprKind::Unary {
                op: UnaryOp::Negate,
                expr,
            } => {
                if let ExprKind::Integer(raw) = &expr.kind {
                    Some((raw.clone(), true))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some((raw, negative)) = literal {
            let ty = hint.unwrap_or(IntegerType::Int64);
            let number = raw
                .replace('_', "")
                .parse::<i128>()
                .ok()
                .and_then(|n| if negative { n.checked_neg() } else { Some(n) });
            if let Some(value) = number.and_then(|n| IntegerValue::new(ty, n)) {
                expr.kind = ExprKind::FixedInteger(value);
            } else {
                self.errors.push(
                    Diagnostic::error(format!(
                        "integer literal is outside the range {}..={} of {}",
                        ty.min(),
                        ty.max(),
                        ty.name()
                    ))
                    .with_span(expr.span.clone()),
                );
            }
            return VoxType::integer(ty);
        }
        match &mut expr.kind {
            ExprKind::IntegerWiden { target, ty } => {
                self.expr(target, None, scope);
                VoxType::integer(*ty)
            }
            ExprKind::FixedInteger(value) => VoxType::integer(value.ty()),
            ExprKind::UInt(_) => VoxType::UInt,
            ExprKind::Float(_) => VoxType::Float,
            ExprKind::Bool(_) => VoxType::Bool,
            ExprKind::Null => unknown(),
            ExprKind::String(literal) => {
                for part in &mut literal.parts {
                    if let StringPart::Interpolation(e) = part {
                        self.expr(e, None, scope);
                    }
                }
                VoxType::String
            }
            ExprKind::Name(name) => scope
                .values
                .get(&name.to_source_string())
                .cloned()
                .unwrap_or_else(unknown),
            ExprKind::Unary { op, expr } => {
                let ty = self.expr(expr, expected, scope);
                if *op == UnaryOp::Not {
                    VoxType::Bool
                } else {
                    ty
                }
            }
            ExprKind::Binary { left, op, right } => {
                let arithmetic = matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Subtract
                        | BinaryOp::Multiply
                        | BinaryOp::Divide
                        | BinaryOp::Remainder
                );
                let preferred = if arithmetic { expected.cloned() } else { None }
                    .or_else(|| {
                        if !unsuffixed(left) {
                            peek(left, scope)
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        if !unsuffixed(right) {
                            peek(right, scope)
                        } else {
                            None
                        }
                    });
                let a = self.expr(left, preferred.as_ref(), scope);
                let b = self.expr(right, preferred.as_ref(), scope);
                if !arithmetic {
                    return if *op == BinaryOp::Coalesce {
                        expected.cloned().unwrap_or(a)
                    } else {
                        VoxType::Bool
                    };
                }
                if a == VoxType::Float || b == VoxType::Float {
                    VoxType::Float
                } else if let Some(common) = a
                    .integer_type()
                    .zip(b.integer_type())
                    .and_then(|(a, b)| a.common(b))
                {
                    VoxType::integer(common)
                } else {
                    a
                }
            }
            ExprKind::Call { callee, arguments } => {
                let label = label(callee);
                let mut signature = scope.functions.get(&label).cloned();
                if let ExprKind::Field { target, name } = &mut callee.kind {
                    let receiver_type = scope
                        .functions
                        .get(name)
                        .and_then(|sig| sig.parameters.first())
                        .cloned();
                    self.expr(target, receiver_type.as_ref(), scope);
                    if signature.is_none() {
                        if let Some(sig) = scope.functions.get(name).cloned() {
                            signature = Some(Signature {
                                names: sig.names.into_iter().skip(1).collect(),
                                parameters: sig.parameters.into_iter().skip(1).collect(),
                                result: sig.result,
                            });
                        }
                    }
                    if let Some(ty) = name.strip_prefix("to").and_then(IntegerType::from_name) {
                        for arg in arguments {
                            self.argument(arg, None, scope);
                        }
                        return VoxType::Nullable(Box::new(VoxType::integer(ty)));
                    }
                }
                for (i, argument) in arguments.iter_mut().enumerate() {
                    let expected = signature.as_ref().and_then(|s| match argument {
                        Argument::Named { name, .. } => s
                            .names
                            .iter()
                            .position(|n| n == name)
                            .and_then(|i| s.parameters.get(i)),
                        _ => s.parameters.get(i),
                    });
                    self.argument(argument, expected, scope);
                }
                signature.map(|s| s.result).unwrap_or_else(unknown)
            }
            ExprKind::NonNull { target } => match self.expr(target, None, scope) {
                VoxType::Nullable(inner) => *inner,
                ty => ty,
            },
            ExprKind::Field { target, name } | ExprKind::SafeField { target, name } => {
                let ty = self.expr(target, None, scope);
                field_type(&ty, name, scope).unwrap_or_else(unknown)
            }
            ExprKind::List(items) => {
                let hint = match expected {
                    Some(VoxType::List(item)) => Some(item.as_ref()),
                    _ => None,
                };
                let mut ty = unknown();
                for item in items.iter_mut() {
                    ty = join(ty, self.expr(item, hint, scope));
                }
                if hint.is_none() && ty.integer_type().is_some() {
                    for item in items.iter_mut() {
                        self.expr(item, Some(&ty), scope);
                    }
                }
                VoxType::List(Box::new(hint.cloned().unwrap_or(ty)))
            }
            ExprKind::Tuple(items) => VoxType::Tuple(
                items
                    .iter_mut()
                    .enumerate()
                    .map(|(i, e)| {
                        self.expr(
                            e,
                            match expected {
                                Some(VoxType::Tuple(items)) => items.get(i),
                                _ => None,
                            },
                            scope,
                        )
                    })
                    .collect(),
            ),
            ExprKind::Record(fields) => VoxType::Record(
                fields
                    .iter_mut()
                    .map(|f| {
                        let explicit = f.ty.as_ref().map(syntax_type);
                        let hint = explicit.as_ref().or_else(|| match expected {
                            Some(VoxType::Record(fields)) => fields
                                .iter()
                                .find(|field| field.name == f.name)
                                .map(|f| &f.ty),
                            _ => None,
                        });
                        RecordField {
                            name: f.name.clone(),
                            ty: self.expr(&mut f.value, hint, scope),
                        }
                    })
                    .collect(),
            ),
            ExprKind::Block(block) => self.block(block, expected, scope),
            ExprKind::If(branches) => {
                let mut ty = unknown();
                for b in &mut branches.branches {
                    self.expr(&mut b.condition, None, scope);
                    ty = join(ty, self.block(&mut b.body, expected, scope));
                }
                if let Some(b) = &mut branches.else_branch {
                    ty = join(ty, self.block(b, expected, scope));
                }
                if expected.is_none() && ty.integer_type().is_some() {
                    for b in &mut branches.branches {
                        self.block(&mut b.body, Some(&ty), scope);
                    }
                    if let Some(b) = &mut branches.else_branch {
                        self.block(b, Some(&ty), scope);
                    }
                }
                ty
            }
            ExprKind::When(when) => {
                self.expr(&mut when.subject, None, scope);
                let mut ty = unknown();
                for arm in &mut when.arms {
                    ty = self.expr(&mut arm.body, expected, scope);
                }
                if let Some(e) = &mut when.else_arm {
                    ty = self.expr(e, expected, scope);
                }
                ty
            }
            ExprKind::Index { target, index } => {
                self.expr(index, Some(&VoxType::Int), scope);
                match self.expr(target, None, scope) {
                    VoxType::List(item) => *item,
                    _ => unknown(),
                }
            }
            ExprKind::Range(range) => {
                if let Some(e) = &mut range.start {
                    self.expr(e, None, scope);
                }
                if let Some(e) = &mut range.end {
                    self.expr(e, None, scope);
                }
                unknown()
            }
            ExprKind::For(for_expr) => {
                let mut nested = scope.clone();
                if let Some(init) = &mut for_expr.init {
                    self.item(init, &mut nested);
                }
                match &mut for_expr.header {
                    ForHeader::In { pattern, iterable } => {
                        if let VoxType::List(item) = self.expr(iterable, None, &mut nested) {
                            nested.values.insert(pattern.clone(), *item);
                        }
                    }
                    ForHeader::Condition(e) => {
                        self.expr(e, None, &mut nested);
                    }
                }
                self.block(&mut for_expr.body, None, &nested);
                unknown()
            }
            ExprKind::Lambda(lambda) => {
                let mut nested = scope.clone();
                for p in &lambda.parameters {
                    if let Some(ty) = &p.ty {
                        nested.values.insert(p.name.clone(), syntax_type(ty));
                    }
                }
                self.expr(&mut lambda.body, None, &mut nested);
                unknown()
            }
            ExprKind::ReceiverCall {
                receiver,
                callee,
                arguments,
            } => {
                self.expr(receiver, None, scope);
                let sig = scope.functions.get(&callee.to_source_string()).cloned();
                for (i, a) in arguments.iter_mut().enumerate() {
                    self.argument(a, sig.as_ref().and_then(|s| s.parameters.get(i + 1)), scope);
                }
                sig.map(|s| s.result).unwrap_or_else(unknown)
            }
            ExprKind::Intrinsic(IntrinsicExpr::Updated(updated)) => {
                let ty = self.expr(&mut updated.target, None, scope);
                for a in &mut updated.updates {
                    let hint =
                        a.ty.as_ref()
                            .map(syntax_type)
                            .or_else(|| update_type(&ty, &a.path));
                    self.expr(&mut a.value, hint.as_ref(), scope);
                }
                ty
            }
            ExprKind::Intrinsic(IntrinsicExpr::Econ(econ)) => {
                self.block(&mut econ.body, Some(&syntax_type(&econ.ty)), scope)
            }
            ExprKind::Integer(_) => unreachable!(),
        }
    }
    fn argument(
        &mut self,
        argument: &mut Argument,
        expected: Option<&VoxType>,
        scope: &mut Context,
    ) {
        match argument {
            Argument::Positional(e) | Argument::Named { value: e, .. } => {
                self.expr(e, expected, scope);
            }
        }
    }
}
fn unsuffixed(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Integer(_))
        || matches!(&e.kind, ExprKind::Unary { op: UnaryOp::Negate, expr } if matches!(expr.kind, ExprKind::Integer(_)))
}
fn label(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Name(n) => n.to_source_string(),
        ExprKind::Field { target, name } => format!("{}.{name}", label(target)),
        _ => String::new(),
    }
}
fn peek(e: &Expr, scope: &Context) -> Option<VoxType> {
    match &e.kind {
        ExprKind::FixedInteger(v) => Some(VoxType::integer(v.ty())),
        ExprKind::UInt(_) => Some(VoxType::UInt),
        ExprKind::Float(_) => Some(VoxType::Float),
        ExprKind::Name(n) => scope.values.get(&n.to_source_string()).cloned(),
        ExprKind::Call { callee, .. } => scope
            .functions
            .get(&label(callee))
            .map(|s| s.result.clone()),
        ExprKind::Unary { expr, .. } => peek(expr, scope),
        _ => None,
    }
}

fn join(a: VoxType, b: VoxType) -> VoxType {
    if a == unknown() {
        return b;
    }
    if b == unknown() {
        return a;
    }
    if let Some(common) = a
        .integer_type()
        .zip(b.integer_type())
        .and_then(|(a, b)| a.common(b))
    {
        VoxType::integer(common)
    } else {
        a
    }
}
fn update_type(ty: &VoxType, path: &[UpdatedPathSegment]) -> Option<VoxType> {
    let Some((first, rest)) = path.split_first() else {
        return Some(ty.clone());
    };
    let next = match (ty, first) {
        (VoxType::Record(fields), UpdatedPathSegment::Field(name)) => {
            &fields.iter().find(|f| f.name == *name)?.ty
        }
        (VoxType::Tuple(items), UpdatedPathSegment::Index(index)) => items.get(*index)?,
        (VoxType::List(item), UpdatedPathSegment::Index(_)) => item.as_ref(),
        _ => return None,
    };
    update_type(next, rest)
}

pub(super) fn field_type(ty: &VoxType, name: &str, scope: &Context) -> Option<VoxType> {
    let fields = match ty {
        VoxType::Record(fields) => fields,
        VoxType::OpaqueSurface(name) => scope.structs.get(name)?,
        VoxType::Named(name) => {
            scope
                .structs
                .get(&format!("{}.{}", name.module.as_str(), name.name))?
        }
        _ => return None,
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.ty.clone())
}
