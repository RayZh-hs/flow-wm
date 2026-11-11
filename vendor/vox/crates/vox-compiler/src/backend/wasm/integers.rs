//! Integer arithmetic stays in Wasm; host calls are confined to error paths.
use super::*;
use vox_core::{builtins::BuiltinReceiver, integer::IntegerType};

fn set_tag(result: MirValueId, ty: IntegerType, ctx: &mut Ctx, f: &mut Function) {
    i32(f, ty.tag());
    local_set(f, ctx.tag_local(result));
    record_known_tag(ctx, result, ty.tag());
}
fn fail_if(
    op: &str,
    args: &[MirValueId],
    result: MirValueId,
    ctx: &mut Ctx,
    f: &mut Function,
) -> Result<(), String> {
    f.instruction(&Instruction::If(BlockType::Empty));
    builtin_op_call(
        BuiltinOp::NumericChecked,
        args,
        &[op.as_bytes().to_vec()],
        result,
        ctx,
        f,
    )?;
    f.instruction(&Instruction::Unreachable);
    f.instruction(&Instruction::End);
    Ok(())
}
fn normalize(ty: IntegerType, f: &mut Function) {
    if ty.bits() <= 32 {
        f.instruction(&Instruction::I32WrapI64);
        match (ty.bits(), ty.signed()) {
            (8, true) => {
                f.instruction(&Instruction::I32Extend8S);
            }
            (16, true) => {
                f.instruction(&Instruction::I32Extend16S);
            }
            (8 | 16, false) => {
                i32(f, ty.max() as i32);
                f.instruction(&Instruction::I32And);
            }
            _ => {}
        }
        f.instruction(&if ty.signed() {
            Instruction::I64ExtendI32S
        } else {
            Instruction::I64ExtendI32U
        });
    }
}
fn bounds(ty: IntegerType, result: MirValueId, ctx: &Ctx, f: &mut Function) {
    local_get(f, ctx.data_local(result));
    i64(f, ty.max() as i64);
    f.instruction(&if ty.signed() {
        Instruction::I64GtS
    } else {
        Instruction::I64GtU
    });
    if ty.signed() {
        local_get(f, ctx.data_local(result));
        i64(f, ty.min() as i64);
        f.instruction(&Instruction::I64LtS);
        f.instruction(&Instruction::I32Or);
    }
}

pub(super) fn binary(
    op: &str,
    left: MirValueId,
    right: MirValueId,
    result: MirValueId,
    ctx: &mut Ctx,
    f: &mut Function,
    body: &MirBody,
) -> Result<(), String> {
    let a = value_type(body, left)
        .and_then(VoxType::integer_type)
        .ok_or("missing integer operand type")?;
    let b = value_type(body, right)
        .and_then(VoxType::integer_type)
        .ok_or("missing integer operand type")?;
    let ty = a
        .common(b)
        .ok_or("integer operands require explicit signed/unsigned conversion")?;
    emit_tag_check_mut(f, ctx, left, a.tag());
    emit_tag_check_mut(f, ctx, right, b.tag());
    set_tag(result, ty, ctx, f);
    if matches!(op, "divide" | "remainder") {
        local_get(f, ctx.data_local(right));
        f.instruction(&Instruction::I64Eqz);
        fail_if(op, &[left, right], result, ctx, f)?;
    }
    // Wasm traps on signed MIN / -1 even when overflow checks are disabled.
    // In wrapping mode this case must produce MIN explicitly.
    let signed_division = ty.signed() && op == "divide";
    if signed_division {
        local_get(f, ctx.data_local(left));
        i64(f, ty.min() as i64);
        f.instruction(&Instruction::I64Eq);
        local_get(f, ctx.data_local(right));
        i64(f, -1);
        f.instruction(&Instruction::I64Eq);
        f.instruction(&Instruction::I32And);
        f.instruction(&Instruction::If(BlockType::Empty));
        if ctx.skip_overflow_check {
            i64(f, ty.min() as i64);
            local_set(f, ctx.data_local(result));
        } else {
            i32(f, 1);
            fail_if(op, &[left, right], result, ctx, f)?;
        }
        f.instruction(&Instruction::Else);
    }
    if !ctx.skip_overflow_check && op == "multiply" && ty.bits() == 64 {
        if !ty.signed() {
            local_get(f, ctx.data_local(right));
            f.instruction(&Instruction::I64Eqz);
            f.instruction(&Instruction::If(BlockType::Empty));
            f.instruction(&Instruction::Else);
            local_get(f, ctx.data_local(left));
            i64(f, -1);
            local_get(f, ctx.data_local(right));
            f.instruction(&Instruction::I64DivU);
            f.instruction(&Instruction::I64GtU);
            fail_if(op, &[left, right], result, ctx, f)?;
            f.instruction(&Instruction::End);
        } else {
            // Avoid trapping while checking the product using signed division.
            local_get(f, ctx.data_local(left));
            i64(f, i64::MIN);
            f.instruction(&Instruction::I64Eq);
            local_get(f, ctx.data_local(right));
            i64(f, -1);
            f.instruction(&Instruction::I64Eq);
            f.instruction(&Instruction::I32And);
            fail_if(op, &[left, right], result, ctx, f)?;
        }
    }
    let narrow_native =
        ty.bits() <= 32 && (ctx.skip_overflow_check || matches!(op, "divide" | "remainder"));
    local_get(f, ctx.data_local(left));
    if narrow_native {
        f.instruction(&Instruction::I32WrapI64);
    }
    local_get(f, ctx.data_local(right));
    if narrow_native {
        f.instruction(&Instruction::I32WrapI64);
    }
    let instruction = match (op, narrow_native, ty.signed()) {
        ("add", true, _) => Instruction::I32Add,
        ("subtract", true, _) => Instruction::I32Sub,
        ("multiply", true, _) => Instruction::I32Mul,
        ("divide", true, true) => Instruction::I32DivS,
        ("divide", true, false) => Instruction::I32DivU,
        ("remainder", true, true) => Instruction::I32RemS,
        ("remainder", true, false) => Instruction::I32RemU,
        ("add", false, _) => Instruction::I64Add,
        ("subtract", false, _) => Instruction::I64Sub,
        ("multiply", false, _) => Instruction::I64Mul,
        ("divide", false, true) => Instruction::I64DivS,
        ("divide", false, false) => Instruction::I64DivU,
        ("remainder", false, true) => Instruction::I64RemS,
        ("remainder", false, false) => Instruction::I64RemU,
        _ => return Err(format!("unsupported integer operator `{op}`")),
    };
    f.instruction(&instruction);
    if narrow_native {
        f.instruction(&if ty.signed() {
            Instruction::I64ExtendI32S
        } else {
            Instruction::I64ExtendI32U
        });
    }
    local_set(f, ctx.data_local(result));
    if !ctx.skip_overflow_check && matches!(op, "add" | "subtract" | "multiply") {
        if ty.bits() < 64 {
            bounds(ty, result, ctx, f);
            fail_if(op, &[left, right], result, ctx, f)?;
        } else if ty.signed() {
            match op {
                "add" | "subtract" => {
                    local_get(f, ctx.data_local(left));
                    local_get(f, ctx.data_local(if op == "add" { result } else { right }));
                    f.instruction(&Instruction::I64Xor);
                    local_get(f, ctx.data_local(if op == "add" { right } else { left }));
                    local_get(f, ctx.data_local(result));
                    f.instruction(&Instruction::I64Xor);
                    f.instruction(&Instruction::I64And);
                    i64(f, 0);
                    f.instruction(&Instruction::I64LtS);
                    fail_if(op, &[left, right], result, ctx, f)?;
                }
                "multiply" => {
                    local_get(f, ctx.data_local(right));
                    f.instruction(&Instruction::I64Eqz);
                    f.instruction(&Instruction::If(BlockType::Empty));
                    f.instruction(&Instruction::Else);
                    local_get(f, ctx.data_local(result));
                    local_get(f, ctx.data_local(right));
                    f.instruction(&Instruction::I64DivS);
                    local_get(f, ctx.data_local(left));
                    f.instruction(&Instruction::I64Ne);
                    fail_if(op, &[left, right], result, ctx, f)?;
                    f.instruction(&Instruction::End);
                }
                _ => {}
            }
        } else if op != "multiply" {
            local_get(f, ctx.data_local(if op == "add" { result } else { left }));
            local_get(f, ctx.data_local(if op == "add" { left } else { right }));
            f.instruction(&Instruction::I64LtU);
            fail_if(op, &[left, right], result, ctx, f)?;
        }
    }
    local_get(f, ctx.data_local(result));
    normalize(ty, f);
    local_set(f, ctx.data_local(result));
    if signed_division {
        f.instruction(&Instruction::End);
    }
    Ok(())
}

pub(super) fn negate(
    arg: MirValueId,
    result: MirValueId,
    ctx: &mut Ctx,
    f: &mut Function,
    body: &MirBody,
) -> Result<(), String> {
    let ty = value_type(body, arg)
        .and_then(VoxType::integer_type)
        .ok_or("missing integer type")?;
    if !ty.signed() {
        return Err("unsigned unary minus is invalid".to_owned());
    }
    emit_tag_check_mut(f, ctx, arg, ty.tag());
    set_tag(result, ty, ctx, f);
    if !ctx.skip_overflow_check {
        local_get(f, ctx.data_local(arg));
        i64(f, ty.min() as i64);
        f.instruction(&Instruction::I64Eq);
        fail_if("negate", &[arg], result, ctx, f)?;
    }
    i64(f, 0);
    local_get(f, ctx.data_local(arg));
    f.instruction(&Instruction::I64Sub);
    normalize(ty, f);
    local_set(f, ctx.data_local(result));
    Ok(())
}

pub(super) fn builtin(
    callee: &str,
    args: &[MirValueId],
    result: MirValueId,
    ctx: &mut Ctx,
    f: &mut Function,
    body: &MirBody,
) -> Result<bool, String> {
    let Some((receiver, method)) = builtins::split_builtin_callee(callee) else {
        return Ok(false);
    };
    if !matches!(
        receiver,
        BuiltinReceiver::Int
            | BuiltinReceiver::UInt
            | BuiltinReceiver::Integer(_)
            | BuiltinReceiver::Float
    ) || args.len() != 1
    {
        return Ok(false);
    }
    let source = wasm_scalar_type(body, args[0]).ok_or("numeric conversion missing source type")?;
    if method == "toFloat" {
        emit_value_as_f64(f, ctx, args[0], source);
        f.instruction(&Instruction::I64ReinterpretF64);
        local_set(f, ctx.data_local(result));
        i32(f, TAG_FLOAT);
        local_set(f, ctx.tag_local(result));
        record_known_tag(ctx, result, TAG_FLOAT);
        return Ok(true);
    }
    let Some(to) = method.strip_prefix("to").and_then(IntegerType::from_name) else {
        return Ok(false);
    };
    if let Some(from) = source.integer_type() {
        i32(f, 1);
        if to.min() > from.min() {
            local_get(f, ctx.data_local(args[0]));
            i64(f, to.min() as i64);
            f.instruction(&if from.signed() {
                Instruction::I64GeS
            } else {
                Instruction::I64GeU
            });
            f.instruction(&Instruction::I32And);
        }
        if to.max() < from.max() {
            local_get(f, ctx.data_local(args[0]));
            i64(f, to.max() as i64);
            f.instruction(&if from.signed() {
                Instruction::I64LeS
            } else {
                Instruction::I64LeU
            });
            f.instruction(&Instruction::I32And);
        }
    } else {
        emit_value_as_f64(f, ctx, args[0], source);
        f.instruction(&Instruction::F64Trunc);
        f.instruction(&Instruction::F64Const(to.min() as f64));
        f.instruction(&Instruction::F64Ge);
        emit_value_as_f64(f, ctx, args[0], source);
        f.instruction(&Instruction::F64Trunc);
        f.instruction(&Instruction::F64Const((to.max() + 1) as f64));
        f.instruction(&Instruction::F64Lt);
        f.instruction(&Instruction::I32And);
    }
    f.instruction(&Instruction::If(BlockType::Empty));
    i32(f, to.tag());
    local_set(f, ctx.tag_local(result));
    if source == WasmScalar::Float {
        emit_value_as_f64(f, ctx, args[0], source);
        f.instruction(&if to.signed() {
            Instruction::I64TruncF64S
        } else {
            Instruction::I64TruncF64U
        });
    } else {
        local_get(f, ctx.data_local(args[0]));
    }
    normalize(to, f);
    local_set(f, ctx.data_local(result));
    f.instruction(&Instruction::Else);
    i32(f, TAG_NULL);
    local_set(f, ctx.tag_local(result));
    i64(f, 0);
    local_set(f, ctx.data_local(result));
    f.instruction(&Instruction::End);
    ctx.known_tags.remove(&result);
    Ok(true)
}
