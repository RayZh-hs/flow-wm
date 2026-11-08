use super::bindings::{python, vox_expression};
use super::*;
use anyhow::bail;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use vox_core::{
    host::{FunctionExportKind, FunctionSpec, PackageManifest, ParameterSpec, Purity},
    opt::OptimizationLevel,
    source::{ModulePath, SourceText},
    types::VoxType,
    value::{InlineValue, RuntimeValue},
};
use vox_runtime::{HostCallArgument, HostFunctionHandler, Runtime};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Activation {
    pub id: String,
    pub args: Value,
    pub inputs: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Evaluation {
    pub values: BTreeMap<String, Record>,
    pub activations: Vec<Activation>,
    pub executed: Vec<String>,
    pub errors: BTreeMap<String, String>,
    pub answer: Value,
}

struct ContextData {
    state: State,
    queue: VecDeque<(String, Value)>,
    cache: HashMap<String, String>,
    visiting: HashSet<String>,
    stack: Vec<String>,
    current_args: Value,
    current_node: String,
    root: String,
    data: bool,
    steps: usize,
    cancel: Arc<AtomicBool>,
    result: Evaluation,
}

/// A fresh evaluation frame is used for every delivered signal. Data fanout
/// shares results within that frame; later signals always request fresh data.
pub fn evaluate(
    state: State,
    root: &str,
    signal: Option<(&str, Value)>,
    cancel: Arc<AtomicBool>,
) -> Result<Evaluation> {
    state.validate()?;
    state.node(root).context("Node not found")?;
    let context = Arc::new(Mutex::new(ContextData {
        state,
        queue: VecDeque::new(),
        cache: HashMap::new(),
        visiting: HashSet::new(),
        stack: vec![],
        current_args: json!({}),
        current_node: root.into(),
        root: root.into(),
        data: signal.is_none(),
        steps: 0,
        cancel,
        result: Evaluation::default(),
    }));
    if let Some((pin, args)) = signal {
        if pin.is_empty() {
            signal_environment(&args)?;
            context.lock().unwrap().queue.push_back((root.into(), args));
        } else {
            context.lock().unwrap().emit(root, pin, args)?;
        }
    }
    let mut runtime = Runtime::default();
    let package = ModulePath::parse("flowwm.host").map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let specs = vec![
        spec(
            "call",
            &[
                VoxType::String,
                VoxType::String,
                VoxType::String,
                VoxType::String,
            ],
            VoxType::String,
        ),
        spec("count", &[VoxType::String, VoxType::String], VoxType::Int),
        spec(
            "at",
            &[VoxType::String, VoxType::String, VoxType::Int],
            VoxType::String,
        ),
    ];
    let manifest = PackageManifest {
        package,
        reexports: vec![],
        types: vec![],
        traits: vec![],
        functions: specs,
        values: vec![],
        trait_impls: BTreeMap::new(),
    };
    let mut handlers = vec![];
    for name in ["call", "count", "at"] {
        let shared = context.clone();
        let handler: HostFunctionHandler = Arc::new(move |_, args| {
            let strings: Vec<_> = args
                .iter()
                .map(argument)
                .collect::<std::result::Result<_, _>>()?;
            let mut context = shared.lock().unwrap();
            if context.cancel.load(Ordering::Relaxed) {
                return Err("Graph evaluation stopped".into());
            }
            let result = match name {
                "call" => {
                    let result = context.call(&strings[0], &strings[1], &strings[2], &strings[3]);
                    match result {
                        Err(error)
                            if matches!(
                                strings[0].as_str(),
                                "local"
                                    | "cast_input"
                                    | "get"
                                    | "publish"
                                    | "snapshot"
                                    | "emit"
                                    | "emit_results"
                                    | "break"
                                    | "merge_inputs"
                            ) =>
                        {
                            let node = context
                                .stack
                                .last()
                                .cloned()
                                .unwrap_or_else(|| context.current_node.clone());
                            context.result.errors.insert(node, format!("{error:#}"));
                            Ok(RuntimeValue::Inline(InlineValue::String("!error".into())))
                        }
                        result => result.map(|s| RuntimeValue::Inline(InlineValue::String(s))),
                    }
                }
                "count" => Ok(RuntimeValue::Inline(InlineValue::Int(
                    if strings[1] == "events" {
                        context.queue.len()
                    } else {
                        context.data_pins(&strings[0], false).len()
                    } as i64,
                ))),
                _ => strings[2]
                    .parse::<usize>()
                    .context("Invalid pin index")
                    .and_then(|i| {
                        context
                            .data_pins(&strings[0], false)
                            .get(i)
                            .map(|p| RuntimeValue::Inline(InlineValue::String(p.name.clone())))
                            .context("Missing pin")
                    }),
            };
            result.map_err(|e| format!("{e:#}"))
        });
        handlers.push((name.to_string(), handler));
    }
    runtime
        .mount_host_library(manifest, handlers)
        .map_err(anyhow::Error::msg)?;
    let artifact = runtime.load_script(
        SourceText::new("flowwm/evaluator.vox", 1, include_str!("evaluator.vox")),
        Some(OptimizationLevel::NOpt),
    )?;
    runtime.run_script(artifact, &[])?;
    let result = std::mem::take(&mut context.lock().unwrap().result);
    Ok(result)
}

fn spec(name: &str, args: &[VoxType], return_type: VoxType) -> FunctionSpec {
    FunctionSpec {
        name: name.into(),
        parameters: args
            .iter()
            .enumerate()
            .map(|(i, ty)| ParameterSpec {
                name: format!("a{i}"),
                ty: ty.clone(),
                has_default: false,
            })
            .collect(),
        return_type,
        purity: Purity::Evil,
        export: FunctionExportKind::Function,
    }
}
fn argument(arg: &HostCallArgument) -> std::result::Result<String, String> {
    match &arg.value {
        Some(RuntimeValue::Inline(InlineValue::String(s))) => Ok(s.clone()),
        Some(RuntimeValue::Inline(InlineValue::Int(i))) => Ok(i.to_string()),
        _ => Err("Invalid graph host argument".into()),
    }
}
fn parse(s: &str) -> Result<Value> {
    serde_json::from_str(s).context("Invalid graph value")
}

impl ContextData {
    fn node(&self, id: &str) -> Result<&Node> {
        self.state.node(id).context("Missing node")
    }
    fn data_pins(&self, id: &str, output: bool) -> Vec<Pin> {
        self.state
            .node(id)
            .map(|n| {
                pins(&self.state, n, output)
                    .into_iter()
                    .filter(|p| !p.ty.is_signal())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn edge(&self, id: &str, pin: &str) -> Option<&Connection> {
        self.state
            .connections
            .iter()
            .find(|e| e.target.node == id && e.target.pin == pin)
    }
    fn emit(&mut self, id: &str, pin: &str, args: Value) -> Result<()> {
        let output = pins(&self.state, self.node(id)?, true)
            .into_iter()
            .find(|p| p.name == pin)
            .context("Missing signal output")?;
        ensure!(output.ty.is_signal(), "Expected a signal output");
        check_value(&self.state, &args, &output.ty)?;
        signal_environment(&args)?;
        for edge in self
            .state
            .connections
            .iter()
            .filter(|e| e.source.node == id && e.source.pin == pin)
        {
            let input = pins(&self.state, self.node(&edge.target.node)?, false)
                .into_iter()
                .find(|p| p.name == edge.target.pin)
                .context("Missing signal input")?;
            let args = match cast_value(&self.state, args.clone(), &output.ty, &input.ty) {
                Ok(args) => args,
                Err(error) => {
                    self.result.errors.insert(
                        edge.target.node.clone(),
                        format!("Signal arguments: {error:#}"),
                    );
                    continue;
                }
            };
            ensure!(self.queue.len() < 4096, "Signal queue limit reached");
            self.queue.push_back((edge.target.node.clone(), args));
        }
        Ok(())
    }
    fn call(&mut self, op: &str, a: &str, b: &str, c: &str) -> Result<String> {
        match op {
            "mode" => Ok(if self.data { "data" } else { "signal" }.into()),
            "root" => Ok(self.root.clone()),
            "cached" => Ok(self.cache.get(a).cloned().unwrap_or_default()),
            "enter" => {
                self.steps += 1;
                ensure!(
                    self.steps <= 4096,
                    "Graph evaluation limit reached (signal loop)"
                );
                ensure!(
                    self.visiting.len() < 128 && self.visiting.insert(a.into()),
                    "Data cycle or evaluation depth limit"
                );
                self.stack.push(a.into());
                Ok(String::new())
            }
            "kind" => Ok(type_id(self.node(a)?).into()),
            "next" => {
                let (node, args) = self.queue.pop_front().context("Empty signal queue")?;
                self.current_args = args;
                self.current_node = node.clone();
                self.cache.clear();
                self.visiting.clear();
                self.stack.clear();
                self.result.executed.push(node.clone());
                Ok(node)
            }
            "event_args" => Ok(self.current_args.to_string()),
            "source_node" => Ok(self
                .edge(a, b)
                .map(|e| e.source.node.clone())
                .unwrap_or_default()),
            "source_pin" => Ok(self
                .edge(a, b)
                .context("Missing data connection")?
                .source
                .pin
                .clone()),
            "property_output" => {
                let node = self.node(a)?;
                let pin = self
                    .data_pins(a, true)
                    .into_iter()
                    .find(|p| p.name == b)
                    .context("Missing output pin")?;
                if let Some(name) = pin.property
                    && let Some(value) = node.flow.values.get(&name).or_else(|| {
                        self.state
                            .node_types
                            .iter()
                            .find(|d| d.id == type_id(node))
                            .and_then(|d| d.properties.get(&name))
                            .filter(|p| p.output)
                            .and_then(|p| p.default.as_ref())
                    })
                {
                    check_value(&self.state, value, &pin.ty)?;
                    return Ok(value.to_string());
                }
                Ok(String::new())
            }
            "get" => Ok(parse(a)?
                .get(b)
                .with_context(|| format!("Missing output: {b}"))?
                .to_string()),
            "put" => {
                let mut object = parse(a)?;
                object
                    .as_object_mut()
                    .context("Expected record")?
                    .insert(b.into(), parse(c)?);
                Ok(object.to_string())
            }
            "local" => {
                let node = self.node(a)?;
                let pin = self
                    .data_pins(a, false)
                    .into_iter()
                    .find(|p| p.name == b)
                    .context("Missing pin")?;
                let value = property(&self.state, node, pin.property.as_deref().unwrap_or(b))
                    .or(pin.default)
                    .with_context(|| format!("Unconnected input: {}.{b}", node.title))?;
                Ok(value.to_string())
            }
            "property" => Ok(property(&self.state, self.node(a)?, b)
                .unwrap_or(parse(c)?)
                .to_string()),
            "constant" => {
                let inputs = parse(b)?;
                let exposed = self
                    .data_pins(a, false)
                    .into_iter()
                    .find(|p| p.property.as_deref() == Some("value"))
                    .and_then(|p| inputs.get(&p.name).cloned());
                Ok(exposed
                    .or_else(|| property(&self.state, self.node(a).ok()?, "value"))
                    .unwrap_or(Value::Null)
                    .to_string())
            }
            "merge_inputs" => {
                let inputs = parse(b)?;
                let mut args = parse(c)?;
                let object = args
                    .as_object_mut()
                    .context("Signal arguments must be a record")?;
                object.extend(
                    inputs
                        .get("args")
                        .and_then(Value::as_object)
                        .context("Merge arguments must be a record")?
                        .clone(),
                );
                for pin in self
                    .data_pins(a, false)
                    .into_iter()
                    .filter(|p| p.name != "args")
                {
                    object.insert(
                        pin.property.unwrap_or_else(|| pin.name.clone()),
                        inputs[&pin.name].clone(),
                    );
                }
                signal_environment(&args)?;
                Ok(args.to_string())
            }
            "cast_input" => {
                let input = self
                    .data_pins(a, false)
                    .into_iter()
                    .find(|p| p.name == b)
                    .context("Missing input")?;
                let from = self
                    .edge(a, b)
                    .and_then(|e| {
                        self.state.node(&e.source.node).and_then(|n| {
                            pins(&self.state, n, true)
                                .into_iter()
                                .find(|p| p.name == e.source.pin)
                        })
                    })
                    .map(|p| p.ty)
                    .unwrap_or(Type::Any);
                Ok(cast_value(&self.state, parse(c)?, &from, &input.ty)?.to_string())
            }
            "merge" => {
                let mut args = parse(a)?;
                let extra = parse(b)?;
                args.as_object_mut()
                    .context("Signal arguments must be a record")?
                    .extend(
                        extra
                            .as_object()
                            .context("Merge arguments must be a record")?
                            .clone(),
                    );
                signal_environment(&args)?;
                Ok(args.to_string())
            }
            "break" => {
                let args = parse(b)?;
                let mut values = BTreeMap::from([("args".to_string(), args.clone())]);
                for pin in self.data_pins(a, true) {
                    if pin.name != "args" {
                        values.insert(
                            pin.name.clone(),
                            args.get(pin.property.as_deref().unwrap_or(&pin.name))
                                .context("Missing signal argument")?
                                .clone(),
                        );
                    }
                }
                Ok(serde_json::to_string(&values)?)
            }
            "snapshot" => {
                let node = self.node(a)?;
                let mut values = node.flow.values.clone();
                values.insert("output".into(), json!(node.output));
                for pin in self.data_pins(a, true) {
                    if let Some(name) = &pin.property
                        && let Some(value) = node.flow.values.get(name)
                    {
                        values.insert(pin.name, value.clone());
                    }
                }
                Ok(serde_json::to_string(&values)?)
            }
            "defer" => {
                self.result.activations.push(Activation {
                    id: a.into(),
                    inputs: parse(b)?,
                    args: parse(c)?,
                });
                Ok("{}".into())
            }
            "invoke" => match self.invoke(a, parse(b)?, parse(c)?) {
                Ok(result) => Ok(result.to_string()),
                Err(error) => {
                    self.result.errors.insert(a.into(), format!("{error:#}"));
                    Ok("!error".into())
                }
            },
            "publish" => {
                if b == "!error" {
                    return Ok(b.into());
                }
                let mut values: Record = serde_json::from_str(b)?;
                let node = self.node(a)?;
                for pin in self.data_pins(a, true) {
                    if let Some(name) = &pin.property
                        && let Some(value) = values
                            .get(name)
                            .or_else(|| node.flow.values.get(name))
                            .cloned()
                    {
                        values.insert(pin.name.clone(), value);
                    }
                    if let Some(value) = values.get(&pin.name) {
                        check_value(&self.state, value, &pin.ty)?;
                    }
                }
                self.state
                    .node_mut(a)
                    .unwrap()
                    .flow
                    .values
                    .extend(values.clone());
                self.result
                    .values
                    .entry(a.into())
                    .or_default()
                    .extend(values.clone());
                Ok(serde_json::to_string(&values)?)
            }
            "store" => {
                self.visiting.remove(a);
                if self.stack.last().is_some_and(|id| id == a) {
                    self.stack.pop();
                }
                self.cache.insert(a.into(), b.into());
                Ok(String::new())
            }
            "emit" => {
                self.emit(a, b, parse(c)?)?;
                Ok(String::new())
            }
            "emit_results" => {
                let node = self.node(a)?;
                let result = parse(b)?;
                let outputs = pins(&self.state, node, true);
                let builtin = matches!(type_id(node), "vox" | "python");
                for pin in outputs.into_iter().filter(|p| p.ty.is_signal()) {
                    if let Some(args) = result.get(&pin.name) {
                        self.emit(a, &pin.name, args.clone())?;
                    } else if builtin && pin.name == "out" {
                        self.emit(a, "out", parse(c)?)?;
                    }
                }
                Ok(String::new())
            }
            "answer" => {
                ensure!(
                    a != "!error",
                    "Node evaluation failed: {}",
                    self.result
                        .errors
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                self.result.answer = parse(a)?;
                Ok(String::new())
            }
            _ => bail!("Unknown graph host operation: {op}"),
        }
    }
    fn invoke(&self, id: &str, mut inputs: Value, args: Value) -> Result<Value> {
        let node = self.node(id)?;
        // Exposed input properties override the local property for this invocation.
        let mut properties = node.flow.properties.clone();
        if let Some(def) = self.state.node_types.iter().find(|d| d.id == type_id(node)) {
            for (name, p) in &def.properties {
                if let Some(value) = &p.default {
                    properties.entry(name.clone()).or_insert(value.clone());
                }
            }
        }
        for pin in self.data_pins(id, false) {
            if let Some(name) = pin.property
                && let Some(value) = inputs.get(&pin.name)
            {
                properties.insert(name, value.clone());
            }
        }
        let bindings = BTreeMap::from([
            ("inputs".into(), std::mem::take(&mut inputs)),
            ("args".into(), args.clone()),
            ("properties".into(), serde_json::to_value(properties)?),
        ]);
        let value = if node.kind == NodeKind::Python {
            python(node, &bindings, &args, &self.cancel)?
        } else {
            let source = self
                .state
                .node_types
                .iter()
                .find(|d| d.id == type_id(node))
                .map(|d| d.source.as_str())
                .unwrap_or(&node.command);
            vox_expression(source, &bindings)?
        };
        ensure!(
            value.is_object(),
            "Node bindings must return an output record"
        );
        Ok(value)
    }
}

/// Signal fields are individual variables. Strings remain raw; other JSON
/// values use JSON encoding, allowing lossless records/lists in subprocesses.
pub fn signal_environment(args: &Value) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for (name, value) in args
        .as_object()
        .context("Signal arguments must be a record")?
    {
        validate_name(name)?;
        let name = format!("FLOWWM_ARG_{}", name.to_ascii_uppercase());
        let value = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        ensure!(!value.contains('\0'), "Signal argument contains NUL");
        ensure!(
            env.insert(name.clone(), value).is_none(),
            "Signal arguments collide: {name}"
        );
    }
    Ok(env)
}
