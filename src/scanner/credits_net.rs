//! Loads the credits-detection graph and reports its inputs and outputs.
//!
//! This is the gate before detection uses the graph. A file that parses
//! is not yet runnable: optimization can still fail, and callers stay on
//! the heuristic until it does not. Nothing here scores a frame.

use std::path::Path;

use tract_tensorflow::prelude::*;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("credits model: {0}")]
    Tract(#[from] TractError),
}

/// One end of the loaded graph: its placeholder name and the fact tract
/// inferred for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outlet {
    pub name: String,
    pub fact: String,
}

/// What the loaded graph accepts and returns, and whether tract could
/// rewrite it into a runnable plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub inputs: Vec<Outlet>,
    pub outputs: Vec<Outlet>,
    /// `None` once the graph optimized. Otherwise the reason it did not,
    /// which means detection cannot run it yet.
    pub optimize_error: Option<String>,
}

/// Loads `path` and returns the graph's inputs and outputs.
///
/// A file that parses is a successful load even when optimization fails.
/// The error stays on the signature so the caller can see the signature
/// and still know inference is not available.
pub fn signature(path: &Path) -> Result<Signature, NetError> {
    let loaded = tract_tensorflow::tensorflow().model_for_path(path)?;
    let inputs = ends(&loaded, loaded.input_outlets()?)?;
    let outputs = ends(&loaded, loaded.output_outlets()?)?;
    let optimize_error = loaded.into_optimized().err().map(|err| err.to_string());
    Ok(Signature {
        inputs,
        outputs,
        optimize_error,
    })
}

fn ends(model: &InferenceModel, ids: &[OutletId]) -> Result<Vec<Outlet>, NetError> {
    ids.iter()
        .map(|id| {
            Ok(Outlet {
                name: model.node(id.node).name.clone(),
                fact: model.outlet_fact(*id)?.format_dt_shape(),
            })
        })
        .collect()
}

impl std::fmt::Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "inputs:")?;
        for outlet in &self.inputs {
            writeln!(f, "  {} {}", outlet.name, outlet.fact)?;
        }
        writeln!(f, "outputs:")?;
        for outlet in &self.outputs {
            writeln!(f, "  {} {}", outlet.name, outlet.fact)?;
        }
        if let Some(err) = &self.optimize_error {
            write!(f, "optimize: {err}")?;
        }
        Ok(())
    }
}
