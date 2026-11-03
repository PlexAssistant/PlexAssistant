//! Loads the credits-detection graph and reports its inputs and outputs.
//!
//! This is the gate before detection uses the graph. A file that parses
//! is not yet runnable: translation can still fail, and callers stay on
//! the heuristic until it does not. Nothing here scores a frame.

use std::path::Path;

use tract_tensorflow::prelude::*;
use tract_tensorflow::tract_hir::internal::*;

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

/// What the loaded graph accepts and returns, and whether it translated
/// into a runnable plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub inputs: Vec<Outlet>,
    pub outputs: Vec<Outlet>,
    /// `None` once the graph translated into a runnable plan. Otherwise
    /// the reason it did not, which means detection cannot run it yet.
    pub optimize_error: Option<String>,
}

/// Loads `path` and returns the graph's inputs and outputs.
///
/// A file that parses is a successful load even when it cannot be run.
/// The error stays on the signature so the caller can see the signature
/// and still know inference is not available.
pub fn signature(path: &Path) -> Result<Signature, NetError> {
    let mut loaded = tract_tensorflow::tensorflow().model_for_path(path)?;
    prepare(&mut loaded)?;
    // The optimizer rewrites a convolution bias it cannot reshape, so a
    // runnable plan is the typed graph itself.
    let optimize_error = match loaded.clone().into_typed().and_then(SimplePlan::new) {
        Ok(plan) => {
            let inputs = ends_typed(plan.model(), plan.model().input_outlets()?)?;
            let outputs = ends_typed(plan.model(), plan.model().output_outlets()?)?;
            return Ok(Signature {
                inputs,
                outputs,
                optimize_error: None,
            });
        }
        Err(err) => Some(format!("{err:#}")),
    };
    let inputs = ends(&loaded, loaded.input_outlets()?)?;
    let outputs = ends(&loaded, loaded.output_outlets()?)?;
    Ok(Signature {
        inputs,
        outputs,
        optimize_error,
    })
}

/// Makes the graph translatable.
///
/// The frame size is fixed to the canvas detection samples onto. Padding
/// and bilinear resize have no translation of their own, so each is
/// replaced by one that reads its constant and hands it to tract's op.
fn prepare(model: &mut InferenceModel) -> Result<(), NetError> {
    model.set_input_fact(0, f32::fact([1, 320, 320, 3]).into())?;
    let pads: Vec<usize> = model
        .nodes()
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.op.name() == "Pad" || node.op.name() == "Unimplemented(ResizeBilinear)"
        })
        .map(|(id, _)| id)
        .collect();
    for id in pads {
        let op: Box<dyn InferenceOp> = if model.node(id).op.name() == "Pad" {
            expand(Pad)
        } else {
            expand(ResizeBilinear)
        };
        model.node_mut(id).op = op;
    }
    Ok(())
}

/// Zero-pads a tensor by the amounts on its second input.
///
/// The amounts are one pair per axis, before then after. The imported
/// padding op has no translation, so this one reads the constant amounts
/// while the graph is translated and hands them to tract's own pad.
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq)]
struct Pad;

impl Expansion for Pad {
    fn name(&self) -> StaticName {
        "Pad".into()
    }

    fn rules<'r, 'p: 'r, 's: 'r>(
        &'s self,
        s: &mut Solver<'r>,
        inputs: &'p [TensorProxy],
        outputs: &'p [TensorProxy],
    ) -> InferenceResult {
        check_input_arity(inputs, 2)?;
        check_output_arity(outputs, 1)?;
        s.equals(&outputs[0].datum_type, &inputs[0].datum_type)?;
        s.equals(&inputs[1].datum_type, DatumType::I32)?;
        s.equals(&inputs[0].rank, &outputs[0].rank)?;
        s.equals(&inputs[1].rank, 2)?;
        s.equals(&inputs[1].shape[0], inputs[0].rank.bex().to_dim())?;
        s.equals(&inputs[1].shape[1], 2.to_dim())?;
        s.given(&inputs[0].rank, move |s, rank| {
            for axis in 0..rank as usize {
                s.equals(
                    &outputs[0].shape[axis],
                    inputs[0].shape[axis].bex()
                        + inputs[1].value[axis][0].bex().to_dim()
                        + inputs[1].value[axis][1].bex().to_dim(),
                )?
            }
            Ok(())
        })
    }

    fn wire(
        &self,
        prefix: &str,
        model: &mut TypedModel,
        inputs: &[OutletId],
    ) -> TractResult<TVec<OutletId>> {
        let Some(paddings) = model.outlet_fact(inputs[1])?.konst.clone() else {
            bail!("pad amounts are not a constant");
        };
        let paddings = paddings
            .to_plain_array_view::<i32>()?
            .into_dimensionality()?;
        let rank = model.outlet_fact(inputs[0])?.rank();
        let pads = (0..rank)
            .map(|axis| (paddings[(axis, 0)] as usize, paddings[(axis, 1)] as usize))
            .collect();
        let op = tract_core::ops::array::Pad {
            pads,
            mode: tract_core::ops::array::PadMode::default(),
        };
        model.wire_node(prefix, op, &[inputs[0]])
    }
}

/// Bilinear resize to the height and width on the second input.
///
/// The size is two integers, height then width, and it covers the spatial
/// axes of an NHWC tensor. Corners are not aligned: a destination pixel
/// maps back half a pixel in, which is what `align_corners` off means.
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq)]
struct ResizeBilinear;

impl Expansion for ResizeBilinear {
    fn name(&self) -> StaticName {
        "ResizeBilinear".into()
    }

    fn rules<'r, 'p: 'r, 's: 'r>(
        &'s self,
        s: &mut Solver<'r>,
        inputs: &'p [TensorProxy],
        outputs: &'p [TensorProxy],
    ) -> InferenceResult {
        check_input_arity(inputs, 2)?;
        check_output_arity(outputs, 1)?;
        s.equals(&outputs[0].datum_type, &inputs[0].datum_type)?;
        s.equals(&inputs[1].datum_type, DatumType::TDim)?;
        s.equals(&inputs[0].rank, 4)?;
        s.equals(&outputs[0].rank, 4)?;
        s.equals(&inputs[1].rank, 1)?;
        s.equals(&inputs[1].shape[0], 2.to_dim())?;
        s.equals(&outputs[0].shape[0], &inputs[0].shape[0])?;
        s.equals(&outputs[0].shape[3], &inputs[0].shape[3])?;
        s.given(&inputs[1].value, move |s, size| {
            let size = size.cast_to::<i64>()?;
            let size = size.try_as_plain_ram()?.to_array_view::<i64>()?;
            s.equals(&outputs[0].shape[1], (size[0] as usize).to_dim())?;
            s.equals(&outputs[0].shape[2], (size[1] as usize).to_dim())
        })
    }

    fn wire(
        &self,
        prefix: &str,
        model: &mut TypedModel,
        inputs: &[OutletId],
    ) -> TractResult<TVec<OutletId>> {
        let Some(size) = model.outlet_fact(inputs[1])?.konst.clone() else {
            bail!("resize size is not a constant");
        };
        // size is height, width; the op wants one length per axis of an NHWC tensor
        let cast = size.cast_to::<i64>()?;
        let view = cast.to_plain_array_view::<i64>()?;
        let Some(hw) = view.as_slice() else {
            bail!("resize size is not contiguous");
        };
        let channels = model.outlet_fact(inputs[0])?.shape[3].to_usize()? as i64;
        let full = tensor1(&[1, hw[0], hw[1], channels]);
        let sizes = model.add_const(format!("{prefix}.size"), full)?;
        let op = tract_core::ops::nn::Resize {
            coord_transformer: tract_core::ops::nn::CoordTransformer::HalfPixel,
            interpolator: tract_core::ops::nn::Interpolator::Linear,
            nearest: tract_core::ops::nn::Nearest::Floor,
            optional_scales_input: None,
            optional_sizes_input: Some(1),
        };
        model.wire_node(prefix, op, &[inputs[0], sizes])
    }
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

fn ends_typed(model: &TypedModel, ids: &[OutletId]) -> Result<Vec<Outlet>, NetError> {
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
