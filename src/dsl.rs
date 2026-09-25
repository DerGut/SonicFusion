//! A small, deliberately fixed vocabulary for building frame execution plans
//! from text. Statements are evaluated in order, so references must be defined
//! before use. The render configuration supplies the final frame limit.

use std::{collections::HashMap, sync::Arc};

use datafusion::physical_plan::{ExecutionPlan, limit::GlobalLimitExec};

use crate::{
    RenderConfig,
    physical::frame::{
        FrameGainExec, FrameLowPassFilterExec, FrameMixExec, FrameSineOscExec, FrameSquareOscExec,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum DslError {
    #[error("DSL at line {line}, column {column}: {message}")]
    Source {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("failed to construct graph: {0}")]
    Plan(#[from] datafusion::error::DataFusionError),
}

type Plan = Arc<dyn ExecutionPlan>;

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Name(String),
    Ref(String),
    Number(f64),
    Arrow,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    Comma,
    Semicolon,
    End,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    at: usize,
}

#[derive(Debug)]
enum Arg {
    Number(f64),
    Ref(String),
    List(Vec<Arg>),
}

#[derive(Debug)]
enum StageKind {
    Call(String, Vec<Arg>),
    Ref(String),
    Out,
}

#[derive(Debug)]
struct Stage {
    kind: StageKind,
    at: usize,
}

/// Parse a graph and instantiate its DataFusion plan using `config`.
///
/// A statement is a `->` chain, optionally ending in a named `$reference` or
/// `out`. Exactly one statement must end in `out`. Bindings are available to
/// subsequent statements. `out` adds the configured finite frame limit.
pub fn build_plan(source: &str, config: &RenderConfig) -> Result<Plan, DslError> {
    let tokens = lex(source)?;
    let statements = Parser::new(source, tokens).parse()?;
    let mut bindings = HashMap::new();
    let mut output = None;

    for stages in statements {
        let mut current: Option<Plan> = None;
        let last = stages.len() - 1;
        for (index, stage) in stages.into_iter().enumerate() {
            match stage.kind {
                StageKind::Call(name, args) => {
                    current = Some(construct(
                        source,
                        stage.at,
                        config,
                        &bindings,
                        current.take(),
                        &name,
                        &args,
                    )?);
                }
                StageKind::Ref(name) if index == 0 => {
                    current = Some(reference(source, stage.at, &bindings, &name)?);
                }
                StageKind::Ref(name) if index == last => {
                    if bindings.contains_key(&name) {
                        return Err(error(
                            source,
                            stage.at,
                            format!("duplicate binding ${name}"),
                        ));
                    }
                    bindings.insert(
                        name,
                        current
                            .take()
                            .ok_or_else(|| error(source, stage.at, "binding requires an input"))?,
                    );
                }
                StageKind::Ref(_) => {
                    return Err(error(source, stage.at, "binding must end a statement"));
                }
                StageKind::Out if index == last => {
                    if output.is_some() {
                        return Err(error(source, stage.at, "graph has more than one out"));
                    }
                    let input = current
                        .take()
                        .ok_or_else(|| error(source, stage.at, "out requires an input"))?;
                    let limit = usize::try_from(config.frame_count())
                        .map_err(|_| error(source, stage.at, "frame_count does not fit usize"))?;
                    output = Some(Arc::new(GlobalLimitExec::new(input, 0, Some(limit))) as Plan);
                }
                StageKind::Out => {
                    return Err(error(source, stage.at, "out must end a statement"));
                }
            }
        }
        if current.is_some() {
            return Err(error(
                source,
                source.len(),
                "statement must end in a binding or out",
            ));
        }
    }

    output.ok_or_else(|| error(source, source.len(), "graph requires one out"))
}

fn construct(
    source: &str,
    at: usize,
    config: &RenderConfig,
    bindings: &HashMap<String, Plan>,
    incoming: Option<Plan>,
    name: &str,
    args: &[Arg],
) -> Result<Plan, DslError> {
    let bad = |message| error(source, at, message);
    match name {
        "sine" | "square" => {
            if incoming.is_some() {
                return Err(bad(format!("{name} is a source and takes no input")));
            }
            let numbers = numeric_args(source, at, name, args, if name == "sine" { 1 } else { 2 })?;
            if name == "sine" {
                Ok(Arc::new(FrameSineOscExec::try_new(config, numbers[0])?))
            } else {
                Ok(Arc::new(FrameSquareOscExec::try_new(
                    config, numbers[0], numbers[1],
                )?))
            }
        }
        "gain" | "lpf" => {
            let (input, params) = unary_args(source, at, bindings, incoming, name, args)?;
            if name == "gain" {
                let factor = f32_value(source, at, params[0], "gain factor")?;
                Ok(Arc::new(FrameGainExec::try_new(config, input, factor)?))
            } else {
                Ok(Arc::new(FrameLowPassFilterExec::try_new(
                    config, input, params[0],
                )?))
            }
        }
        "mix" => {
            let (inputs, gains) = mix_args(source, at, bindings, incoming, args)?;
            Ok(Arc::new(FrameMixExec::try_new(config, inputs, gains)?))
        }
        _ => Err(bad(format!("unknown node {name:?}"))),
    }
}

fn numeric_args(
    source: &str,
    at: usize,
    name: &str,
    args: &[Arg],
    count: usize,
) -> Result<Vec<f64>, DslError> {
    if args.len() != count {
        return Err(error(
            source,
            at,
            format!("{name} expects {count} numeric argument(s)"),
        ));
    }
    args.iter()
        .map(|arg| match arg {
            Arg::Number(n) if n.is_finite() => Ok(*n),
            _ => Err(error(
                source,
                at,
                format!("{name} expects numeric arguments"),
            )),
        })
        .collect()
}

fn unary_args(
    source: &str,
    at: usize,
    bindings: &HashMap<String, Plan>,
    incoming: Option<Plan>,
    name: &str,
    args: &[Arg],
) -> Result<(Plan, Vec<f64>), DslError> {
    let (input, params) = match (incoming, args) {
        (Some(input), [Arg::Number(value)]) => (input, vec![*value]),
        (None, [Arg::Ref(reference_name), Arg::Number(value)]) => (
            reference(source, at, bindings, reference_name)?,
            vec![*value],
        ),
        _ => {
            return Err(error(
                source,
                at,
                format!("{name} expects one input and one numeric argument"),
            ));
        }
    };
    if !params[0].is_finite() {
        return Err(error(source, at, format!("{name} argument must be finite")));
    }
    Ok((input, params))
}

fn mix_args(
    source: &str,
    at: usize,
    bindings: &HashMap<String, Plan>,
    incoming: Option<Plan>,
    args: &[Arg],
) -> Result<(Vec<Plan>, Vec<f32>), DslError> {
    let mut inputs: Vec<Plan> = incoming.into_iter().collect();
    let mut gains = None;
    for (index, arg) in args.iter().enumerate() {
        match arg {
            Arg::Ref(name) if gains.is_none() => {
                inputs.push(reference(source, at, bindings, name)?)
            }
            Arg::List(items)
                if !items.is_empty()
                    && items.iter().all(|item| matches!(item, Arg::Ref(_)))
                    && gains.is_none() =>
            {
                for item in items {
                    if let Arg::Ref(name) = item {
                        inputs.push(reference(source, at, bindings, name)?);
                    }
                }
            }
            Arg::List(items) if index == args.len() - 1 && gains.is_none() => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    let Arg::Number(value) = item else {
                        return Err(error(source, at, "mix weights must be numbers"));
                    };
                    values.push(f32_value(source, at, *value, "mix weight")?);
                }
                gains = Some(values);
            }
            _ => {
                return Err(error(
                    source,
                    at,
                    "mix expects input references followed by an optional weight list",
                ));
            }
        }
    }
    if inputs.is_empty() {
        return Err(error(source, at, "mix requires at least one input"));
    }
    let gains = gains.unwrap_or_else(|| vec![1.0; inputs.len()]);
    if gains.len() != inputs.len() {
        return Err(error(
            source,
            at,
            format!(
                "mix has {} inputs but {} weights",
                inputs.len(),
                gains.len()
            ),
        ));
    }
    Ok((inputs, gains))
}

fn reference(
    source: &str,
    at: usize,
    bindings: &HashMap<String, Plan>,
    name: &str,
) -> Result<Plan, DslError> {
    bindings
        .get(name)
        .cloned()
        .ok_or_else(|| error(source, at, format!("undefined reference ${name}")))
}

fn f32_value(source: &str, at: usize, value: f64, label: &str) -> Result<f32, DslError> {
    let converted = value as f32;
    if !converted.is_finite() {
        return Err(error(source, at, format!("{label} exceeds f32 range")));
    }
    Ok(converted)
}

fn error(source: &str, at: usize, message: impl Into<String>) -> DslError {
    let before = &source[..at];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    DslError::Source {
        line,
        column,
        message: message.into(),
    }
}

fn lex(source: &str) -> Result<Vec<Token>, DslError> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '#' {
            while let Some((_, next)) = chars.peek() {
                if *next == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        let kind = match ch {
            '(' => TokenKind::OpenParen,
            ')' => TokenKind::CloseParen,
            '[' => TokenKind::OpenBracket,
            ']' => TokenKind::CloseBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            '-' if chars.peek().is_some_and(|(_, next)| *next == '>') => {
                chars.next();
                TokenKind::Arrow
            }
            '$' => {
                let mut name = String::new();
                while let Some((_, next)) = chars.peek() {
                    if next.is_ascii_alphanumeric() || *next == '_' {
                        name.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
                    return Err(error(source, at, "expected identifier after $"));
                }
                TokenKind::Ref(name)
            }
            ch if ch.is_ascii_alphabetic() || ch == '_' => {
                let mut name = ch.to_string();
                while let Some((_, next)) = chars.peek() {
                    if next.is_ascii_alphanumeric() || *next == '_' {
                        name.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                TokenKind::Name(name)
            }
            ch if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' => {
                let mut literal = ch.to_string();
                while let Some((_, next)) = chars.peek() {
                    if next.is_ascii_digit()
                        || *next == '.'
                        || *next == 'e'
                        || *next == 'E'
                        || *next == '-'
                        || *next == '+'
                    {
                        literal.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value = literal
                    .parse::<f64>()
                    .map_err(|_| error(source, at, format!("invalid number {literal:?}")))?;
                if !value.is_finite() {
                    return Err(error(source, at, "number must be finite"));
                }
                TokenKind::Number(value)
            }
            _ => return Err(error(source, at, format!("unexpected character {ch:?}"))),
        };
        tokens.push(Token { kind, at });
    }
    tokens.push(Token {
        kind: TokenKind::End,
        at: source.len(),
    });
    Ok(tokens)
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    index: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: Vec<Token>) -> Self {
        Self {
            source,
            tokens,
            index: 0,
        }
    }

    fn parse(mut self) -> Result<Vec<Vec<Stage>>, DslError> {
        let mut statements = Vec::new();
        while self.kind() != &TokenKind::End {
            let mut stages = vec![self.stage()?];
            while self.kind() == &TokenKind::Arrow {
                self.index += 1;
                stages.push(self.stage()?);
            }
            statements.push(stages);
            if self.kind() == &TokenKind::Semicolon {
                self.index += 1;
            } else if self.kind() != &TokenKind::End {
                return self.fail("expected ';' between statements");
            }
        }
        Ok(statements)
    }

    fn stage(&mut self) -> Result<Stage, DslError> {
        let at = self.tokens[self.index].at;
        let kind = match self.kind().clone() {
            TokenKind::Ref(name) => {
                self.index += 1;
                StageKind::Ref(name)
            }
            TokenKind::Name(name) => {
                self.index += 1;
                if name == "out" && self.kind() != &TokenKind::OpenParen {
                    StageKind::Out
                } else {
                    self.expect(TokenKind::OpenParen, "expected '(' after node name")?;
                    let mut args = Vec::new();
                    if self.kind() != &TokenKind::CloseParen {
                        loop {
                            args.push(self.arg()?);
                            if self.kind() != &TokenKind::Comma {
                                break;
                            }
                            self.index += 1;
                        }
                    }
                    self.expect(TokenKind::CloseParen, "expected ')' after arguments")?;
                    StageKind::Call(name, args)
                }
            }
            _ => return self.fail("expected node or $reference"),
        };
        Ok(Stage { kind, at })
    }

    fn arg(&mut self) -> Result<Arg, DslError> {
        let arg = match self.kind().clone() {
            TokenKind::Number(value) => Arg::Number(value),
            TokenKind::Ref(name) => Arg::Ref(name),
            TokenKind::OpenBracket => {
                self.index += 1;
                let mut items = Vec::new();
                if self.kind() != &TokenKind::CloseBracket {
                    loop {
                        items.push(self.arg()?);
                        if self.kind() != &TokenKind::Comma {
                            break;
                        }
                        self.index += 1;
                    }
                }
                self.expect(TokenKind::CloseBracket, "expected ']' after list")?;
                return Ok(Arg::List(items));
            }
            _ => return self.fail("expected number, $reference, or list"),
        };
        self.index += 1;
        Ok(arg)
    }

    fn kind(&self) -> &TokenKind {
        &self.tokens[self.index].kind
    }

    fn expect(&mut self, expected: TokenKind, message: &str) -> Result<(), DslError> {
        if *self.kind() != expected {
            return self.fail(message);
        }
        self.index += 1;
        Ok(())
    }

    fn fail<T>(&self, message: &str) -> Result<T, DslError> {
        Err(error(self.source, self.tokens[self.index].at, message))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use datafusion::{
        execution::TaskContext,
        physical_plan::{collect, limit::GlobalLimitExec},
    };

    use crate::{
        RenderConfig, decode_from_frames,
        physical::frame::{FrameGainExec, FrameLowPassFilterExec, FrameMixExec, FrameSineOscExec},
    };

    use super::build_plan;

    #[tokio::test]
    async fn weighted_branches_match_the_rust_plan() {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(8)
            .batch_frame_capacity(3)
            .build()
            .unwrap();
        let graph = "sine(1) -> $A; sine(2) -> $B; \
                     mix([$A, $B], [0.4, 0.6]) -> lpf(3) -> gain(0.5) -> out";
        let parsed = build_plan(graph, &config).unwrap();
        let a = Arc::new(FrameSineOscExec::try_new(&config, 1.0).unwrap());
        let b = Arc::new(FrameSineOscExec::try_new(&config, 2.0).unwrap());
        let mix = Arc::new(FrameMixExec::try_new(&config, vec![a, b], vec![0.4, 0.6]).unwrap());
        let filter = Arc::new(FrameLowPassFilterExec::try_new(&config, mix, 3.0).unwrap());
        let gain = Arc::new(FrameGainExec::try_new(&config, filter, 0.5).unwrap());
        let expected = Arc::new(GlobalLimitExec::new(gain, 0, Some(8)));
        let parsed_batches = collect(parsed, Arc::new(TaskContext::default()))
            .await
            .unwrap();
        let expected_batches = collect(expected, Arc::new(TaskContext::default()))
            .await
            .unwrap();
        assert_eq!(
            decode_from_frames(&parsed_batches, &config).unwrap(),
            decode_from_frames(&expected_batches, &config).unwrap()
        );
    }

    #[tokio::test]
    async fn implicit_and_explicit_unary_inputs_are_equivalent() {
        let config = RenderConfig::builder()
            .sample_rate_hz(8)
            .frame_count(8)
            .build()
            .unwrap();
        let chained = build_plan("sine(1) -> gain(0.5) -> out", &config).unwrap();
        let referenced = build_plan("sine(1) -> $A; gain($A, 0.5) -> out", &config).unwrap();
        let chained = collect(chained, Arc::new(TaskContext::default()))
            .await
            .unwrap();
        let referenced = collect(referenced, Arc::new(TaskContext::default()))
            .await
            .unwrap();
        assert_eq!(
            decode_from_frames(&chained, &config).unwrap(),
            decode_from_frames(&referenced, &config).unwrap()
        );
    }

    #[test]
    fn rejects_invalid_graphs_with_locations() {
        let config = RenderConfig::default();
        for (graph, message) in [
            (
                "sine(440) -> $A;\nmix($A, $B) -> out",
                "line 2, column 1: undefined reference $B",
            ),
            (
                "sine(440) -> $A; sine(220) -> $A; $A -> out",
                "duplicate binding $A",
            ),
            ("sine(440) -> out; sine(220) -> out", "more than one out"),
            (
                "sine(440) -> gain(0.5)",
                "statement must end in a binding or out",
            ),
            (
                "sine(440) -> $A; mix([$A], [0.4, 0.6]) -> out",
                "mix has 1 inputs but 2 weights",
            ),
            (
                "sine(440) -> lpf(100000) -> out",
                "cutoff_hz must be below Nyquist",
            ),
            ("sine(440) -> out -> gain(0.5)", "out must end a statement"),
            (
                "sine(440) -> $A\nsine(220) -> out",
                "expected ';' between statements",
            ),
        ] {
            let error = build_plan(graph, &config).unwrap_err().to_string();
            assert!(error.contains(message), "{graph:?}: {error}");
        }
    }
}
