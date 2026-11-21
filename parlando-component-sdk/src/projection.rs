//! Reusable policy reads over declared component observations.

use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};

use crate::{
    wire::{Decoder, Token},
    ComponentValue, Schema, VariantPayload,
};

/// The named fields, tuple positions, and list elements a policy wants to read.
///
/// Use an enum as `K` to identify selected values in the policy callback.
/// A selection uses public field names, and [`Projection::resolve`] translates
/// them into wire positions before the policy starts observing values.
pub enum Selection<'a, K> {
    /// Selects fields of a declared struct by their public names.
    Fields(&'a [(&'a str, Self)]),
    /// Selects tuple elements by zero-based position.
    Tuple(&'a [(usize, Self)]),
    /// Applies a selection to every element of a declared list in order.
    List(&'a Self),
    /// Applies the inner selection to a present option. Null emits no values.
    Optional(&'a Self),
    /// Reads one Boolean.
    Bool(K),
    /// Reads one signed integer.
    I64(K),
    /// Reads one unsigned integer.
    U64(K),
    /// Reads one finite float.
    F64(K),
    /// Borrows one UTF-8 string from the observation.
    String(K),
    /// Borrows the selected variant's public name from the declaration.
    EnumLabel(K),
}

/// A selected scalar delivered to a policy during a projection read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProjectedValue<'a> {
    /// A Boolean value.
    Bool(bool),
    /// A signed integer value.
    I64(i64),
    /// An unsigned integer value.
    U64(u64),
    /// A finite floating-point value.
    F64(f64),
    /// A borrowed UTF-8 string from the input bytes.
    String(&'a str),
    /// A borrowed variant name from the resolved enum declaration.
    EnumLabel(&'a str),
}

/// A policy selection resolved against one observation declaration.
///
/// A plan is immutable, can be reused across turns, and owns the schema
/// identity it expects on input values. Compatible declarations with different
/// field or variant order can each have their own plan for the same selection.
///
/// ```
/// use parlando_component_sdk::{ComponentValue, Field, ProjectedValue, Projection, Schema, Selection};
/// use std::sync::Arc;
///
/// # fn main() -> anyhow::Result<()> {
/// let schema = Arc::new(Schema::Struct(vec![Field {
///     name: "floor".into(),
///     schema: Schema::U64,
///     omit_none: false,
/// }]));
/// // A component game supplies this value and the matching seat declaration.
/// let view = ComponentValue::from_schema_bytes(vec![0x91, 3], Arc::clone(&schema))?;
/// let plan = Projection::resolve(schema, &Selection::Fields(&[("floor", Selection::U64(()))]))?;
/// plan.read(&view, |(), value| {
///     assert_eq!(value, ProjectedValue::U64(3));
///     Ok(())
/// })?;
/// # Ok(())
/// # }
/// ```
pub struct Projection<K> {
    schema: Arc<Schema>,
    plan: Plan<K>,
}

/// An executable positional read with no field-name lookups.
enum Plan<K> {
    Slots {
        len: usize,
        children: Vec<(usize, Plan<K>)>,
    },
    List(Box<Plan<K>>),
    Optional(Box<Plan<K>>),
    Scalar {
        key: K,
        kind: Scalar,
    },
    EnumLabel {
        key: K,
        all_unit: bool,
        variants: Vec<(String, usize)>,
    },
}

/// The matching wire token for a scalar policy input.
#[derive(Clone, Copy)]
enum Scalar {
    Bool,
    I64,
    U64,
    F64,
    String,
}

impl<K: Copy> Projection<K> {
    /// Resolves and checks a policy selection against the guest's declaration.
    ///
    /// Pass the seat's schema from [`crate::GameSchema`]. Unknown fields,
    /// repeated positions, and incompatible shapes are reported here, before
    /// any observations reach the policy.
    pub fn resolve(schema: Arc<Schema>, selection: &Selection<'_, K>) -> Result<Self> {
        schema.validate()?;
        let plan = Plan::resolve(&schema, selection)?;
        Ok(Self { schema, plan })
    }

    /// Streams the selected values in wire order using one bounded cursor.
    ///
    /// Named MessagePack values and values from an incompatible declaration
    /// return an error. Skipped values receive structural checks, while selected
    /// values also receive type checks. A callback may have received earlier
    /// values when a later input fails. Commit policy state after successful
    /// completion if the update must be atomic.
    pub fn read<'a>(
        &'a self,
        value: &'a ComponentValue,
        mut visit: impl FnMut(K, ProjectedValue<'a>) -> Result<()>,
    ) -> Result<()> {
        let schema = value
            .schema
            .as_ref()
            .context("projection requires a declared value")?;
        ensure!(
            Arc::ptr_eq(&self.schema, schema) || self.schema.as_ref() == schema.as_ref(),
            "projection resolved for a different component schema"
        );
        let mut decoder = Decoder::new(value.as_messagepack());
        self.plan.read(&mut decoder, 0, &mut visit)?;
        ensure!(decoder.is_finished(), "trailing compact projection data");
        Ok(())
    }
}

impl<K: Copy> Plan<K> {
    /// Checks selected nodes against their declarations and compiles their wire operations.
    fn resolve(schema: &Schema, selection: &Selection<'_, K>) -> Result<Self> {
        match (schema, selection) {
            (Schema::Struct(fields), Selection::Fields(selected)) => {
                let children = selected.iter().map(|(name, selection)| {
                    let index = fields
                        .iter()
                        .position(|field| field.name == *name)
                        .with_context(|| format!("unknown projected component field {name:?}"))?;
                    let plan = Self::resolve(&fields[index].schema, selection)
                        .with_context(|| format!("invalid projection for field {name:?}"))?;
                    Ok((index, plan))
                });
                Self::slots(fields.len(), children)
            }
            (Schema::Tuple(items), Selection::Tuple(selected)) => {
                let children = selected.iter().map(|(index, selection)| {
                    let item = items
                        .get(*index)
                        .with_context(|| format!("unknown projected tuple position {index}"))?;
                    let plan = Self::resolve(item, selection).with_context(|| {
                        format!("invalid projection for tuple position {index}")
                    })?;
                    Ok((*index, plan))
                });
                Self::slots(items.len(), children)
            }
            (Schema::List(item), Selection::List(selection)) => {
                Ok(Self::List(Box::new(Self::resolve(item, selection)?)))
            }
            (Schema::Option(item), Selection::Optional(selection)) => {
                Ok(Self::Optional(Box::new(Self::resolve(item, selection)?)))
            }
            (Schema::Enum { variants, .. }, Selection::EnumLabel(key)) => Ok(Self::EnumLabel {
                key: *key,
                all_unit: variants
                    .iter()
                    .all(|v| matches!(v.payload, VariantPayload::Unit)),
                variants: variants
                    .iter()
                    .map(|v| (v.name.clone(), v.payload.len()))
                    .collect(),
            }),
            (Schema::Bool, Selection::Bool(key)) => Ok(Self::scalar(*key, Scalar::Bool)),
            (Schema::I64, Selection::I64(key)) => Ok(Self::scalar(*key, Scalar::I64)),
            (Schema::U64, Selection::U64(key)) => Ok(Self::scalar(*key, Scalar::U64)),
            (Schema::F64, Selection::F64(key)) => Ok(Self::scalar(*key, Scalar::F64)),
            (Schema::String, Selection::String(key)) => Ok(Self::scalar(*key, Scalar::String)),
            _ => bail!("projection selection differs from its declared shape"),
        }
    }

    /// Builds the fixed array operation and orders selected children by wire position.
    fn slots(len: usize, children: impl Iterator<Item = Result<(usize, Self)>>) -> Result<Self> {
        let mut children: Vec<_> = children.collect::<Result<_>>()?;
        children.sort_unstable_by_key(|(index, _)| *index);
        ensure!(
            children.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "duplicate component projection position"
        );
        Ok(Self::Slots { len, children })
    }

    /// Binds a primitive type check to a policy input key.
    fn scalar(key: K, kind: Scalar) -> Self {
        Self::Scalar { key, kind }
    }

    /// Consumes one complete compact value under the decoder's shared budget.
    fn read<'a>(
        &'a self,
        decoder: &mut Decoder<'a>,
        depth: usize,
        visit: &mut impl FnMut(K, ProjectedValue<'a>) -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Slots { len, children } => {
                ensure!(
                    array_len(decoder, depth)? == *len,
                    "invalid compact projection array length"
                );
                let mut next = 0;
                for (index, child) in children {
                    for _ in next..*index {
                        decoder.skip_bounded(depth + 1)?;
                    }
                    child.read(decoder, depth + 1, visit)?;
                    next = index + 1;
                }
                for _ in next..*len {
                    decoder.skip_bounded(depth + 1)?;
                }
            }
            Self::List(child) => {
                for _ in 0..array_len(decoder, depth)? {
                    child.read(decoder, depth + 1, visit)?;
                }
            }
            Self::Optional(child) => {
                if decoder.next_is_nil() {
                    decoder.next()?;
                } else {
                    child.read(decoder, depth, visit)?;
                }
            }
            Self::Scalar { key, kind } => visit(*key, kind.read(decoder.next()?)?)?,
            Self::EnumLabel {
                key,
                all_unit,
                variants,
            } => {
                let fields = if *all_unit {
                    0
                } else {
                    array_len(decoder, depth)?
                        .checked_sub(1)
                        .context("missing compact enum ordinal")?
                };
                let ordinal = match decoder.next()? {
                    Token::Unsigned(value) => usize::try_from(value),
                    Token::Signed(value) => usize::try_from(value),
                    _ => bail!("invalid compact enum ordinal"),
                }?;
                let (name, expected) = variants
                    .get(ordinal)
                    .context("invalid compact enum ordinal")?;
                ensure!(fields == *expected, "invalid compact enum field count");
                for _ in 0..fields {
                    decoder.skip_bounded(depth + 1)?;
                }
                visit(*key, ProjectedValue::EnumLabel(name))?;
            }
        }
        Ok(())
    }
}

impl Scalar {
    /// Converts a selected token using the component schema's scalar rules.
    fn read(self, token: Token<'_>) -> Result<ProjectedValue<'_>> {
        match (self, token) {
            (Self::Bool, Token::Bool(value)) => Ok(ProjectedValue::Bool(value)),
            (Self::I64, Token::Signed(value)) => Ok(ProjectedValue::I64(value)),
            (Self::I64, Token::Unsigned(value)) => Ok(ProjectedValue::I64(value.try_into()?)),
            (Self::U64, Token::Unsigned(value)) => Ok(ProjectedValue::U64(value)),
            (Self::U64, Token::Signed(value)) => Ok(ProjectedValue::U64(value.try_into()?)),
            (Self::F64, Token::Float(value)) if value.is_finite() => Ok(ProjectedValue::F64(value)),
            (Self::String, Token::String(value)) => Ok(ProjectedValue::String(value)),
            _ => bail!("invalid compact projection scalar type"),
        }
    }
}

/// Opens one compact array and checks resources before walking its elements.
fn array_len(decoder: &mut Decoder<'_>, depth: usize) -> Result<usize> {
    let Token::Array(len) = decoder.next()? else {
        bail!("expected a compact projection array");
    };
    decoder.check_container(len, depth, 1)?;
    Ok(len)
}

#[cfg(test)]
mod tests;
