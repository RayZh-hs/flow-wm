//! Source locations shared by inferred Vox ports and editor usage highlighting.
use std::collections::BTreeSet;
use vox_compiler::frontend::{
    analyze_source,
    ast::*,
    lexer::{LexedStringPart, Lexer, Token, TokenKind},
};
use vox_core::source::SourceText;

fn input_references(source: &str, offset: usize, references: &mut Vec<(String, (usize, usize))>) {
    let (tokens, _) = Lexer::new(source, offset).lex_lossy();
    for (index, token) in tokens.iter().enumerate() {
        if matches!(&token.kind, TokenKind::Identifier(name) if name == "inputs")
            && !index
                .checked_sub(1)
                .and_then(|i| tokens.get(i))
                .is_some_and(|token| matches!(token.kind, TokenKind::Dot | TokenKind::QuestionDot))
            && let Some(Token {
                kind: TokenKind::Dot | TokenKind::QuestionDot,
                ..
            }) = tokens.get(index + 1)
            && let Some(Token {
                kind: TokenKind::Identifier(name),
                span,
            }) = tokens.get(index + 2)
        {
            references.push((name.clone(), (token.span.start, span.end)));
        }
        if let TokenKind::StringLiteral(string) = &token.kind {
            for part in &string.parts {
                if let LexedStringPart::InterpolationExpr { source, span } = part {
                    input_references(source, span.start, references);
                }
            }
        }
    }
}

pub fn vox_inputs(source: &str) -> BTreeSet<String> {
    let mut references = vec![];
    input_references(source, 0, &mut references);
    references.into_iter().map(|(name, _)| name).collect()
}

/// Byte offsets into the original source; comments and ordinary strings are not references.
pub fn vox_pin_spans(source: &str, name: &str, output: bool) -> Vec<(usize, usize)> {
    if !output {
        let mut references = vec![];
        input_references(source, 0, &mut references);
        if name == "in" {
            let (tokens, _) = Lexer::new(source, 0).lex_lossy();
            return tokens
                .iter()
                .filter(
                    |token| matches!(&token.kind, TokenKind::Identifier(name) if name == "args"),
                )
                .map(|token| (token.span.start, token.span.end))
                .collect();
        }
        return references
            .into_iter()
            .filter(|(field, _)| field == name)
            .map(|(_, span)| span)
            .collect();
    }
    const HEADER: &str = "script flowwm.node;\n";
    let Ok(unit) = analyze_source(&SourceText::new(
        "flowwm/node.vox",
        1,
        format!("{HEADER}{source}"),
    )) else {
        return vec![];
    };
    let mut spans = vec![];
    fn result(expr: &Expr, name: &str, spans: &mut Vec<(usize, usize)>) {
        match &expr.kind {
            ExprKind::Record(fields) => {
                for field in fields.iter().filter(|field| field.name == name) {
                    // Include the returned field and the expression that produces its value.
                    spans.push((field.span.start, field.value.span.end));
                }
            }
            ExprKind::Block(block) => {
                if let Some(expr) = &block.trailing {
                    result(expr, name, spans);
                }
                for item in &block.items {
                    if let BlockItem::Return(ReturnStatement {
                        value: Some(expr), ..
                    }) = item
                    {
                        result(expr, name, spans);
                    }
                }
            }
            ExprKind::If(value) => {
                for branch in &value.branches {
                    if let Some(expr) = &branch.body.trailing {
                        result(expr, name, spans);
                    }
                }
                if let Some(body) = &value.else_branch
                    && let Some(expr) = &body.trailing
                {
                    result(expr, name, spans);
                }
            }
            ExprKind::When(value) => {
                for arm in &value.arms {
                    result(&arm.body, name, spans);
                }
                if let Some(expr) = &value.else_arm {
                    result(expr, name, spans);
                }
            }
            _ => {}
        }
    }
    if let Some(expr) = &unit.syntax.result {
        result(expr, name, &mut spans);
    }
    spans
        .into_iter()
        .filter_map(|(start, end)| {
            Some((
                start.checked_sub(HEADER.len())?,
                end.checked_sub(HEADER.len())?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inputs_are_derived_from_code_and_string_interpolations_without_matching_comments() {
        let source = r#"{ // inputs.fake
            val message = "inputs.literal ${inputs.live}";
            { result = inputs.value + inputs.value, other = item.inputs.ignored, }
        }"#;
        assert_eq!(vox_inputs(source), ["live".into(), "value".into()].into());
        let references = vox_pin_spans(source, "value", false);
        assert_eq!(references.len(), 2);
        assert!(
            references
                .iter()
                .all(|&(a, b)| &source[a..b] == "inputs.value")
        );
        let references = vox_pin_spans(source, "live", false);
        assert_eq!(&source[references[0].0..references[0].1], "inputs.live");
    }
    #[test]
    fn output_hover_uses_the_returned_record_instead_of_unrelated_local_fields() {
        let source = "{ val other = { value = 0, }; { value = inputs.x + 2, next = inputs.x, } }";
        let references = vox_pin_spans(source, "value", true);
        assert_eq!(references.len(), 1);
        assert_eq!(
            &source[references[0].0..references[0].1],
            "value = inputs.x + 2"
        );
        assert!(vox_pin_spans(source, "missing", true).is_empty());
    }
    #[test]
    fn incomplete_code_still_discovers_input_fields() {
        assert_eq!(
            vox_inputs("{ value = inputs.payload."),
            ["payload".into()].into()
        );
    }
}
