//! Adapts complete compact view payloads for eager or deferred typed decoding.

use std::{borrow::Cow, marker::PhantomData, sync::Arc};

use crate::{decode_declared, ComponentValue, DecodeGameValue, Schema};

use super::Result;

/// The complete role-private view delivered to a policy.
///
/// A game type implementing [`DecodeGameValue`] is decoded before the agent
/// callback. Use [`LazyView<T>`] when a policy can process some transitions
/// using the action and message alone.
pub trait AgentView: Send + Sized {
    /// Declares the game's compact layout for this view.
    fn view_schema() -> Schema;

    /// Adapts the whole view after resolving its source representation.
    #[doc(hidden)]
    fn receive(source: ViewSource) -> Result<Self>;
}

impl<T: for<'de> DecodeGameValue<'de> + Send> AgentView for T {
    /// Uses the compact layout declared by the typed game value.
    fn view_schema() -> Schema {
        T::schema()
    }

    /// Decodes the complete view before entering the callback.
    fn receive(source: ViewSource) -> Result<Self> {
        source.decode()
    }
}

/// Owns a compact view until a policy requests its typed value.
///
/// Choose `type View = LazyView<MyView>` on an [`super::Agent`] and call
/// [`Self::decode`] wherever the policy needs the view. The game layout is
/// unchanged. Incoming decode errors surface at that call. Dropping the view
/// without decoding avoids reading any of its compact fields.
pub struct LazyView<T> {
    source: ViewSource,
    view: PhantomData<fn() -> T>,
}

impl<T: for<'de> DecodeGameValue<'de>> LazyView<T> {
    /// Consumes the compact view and decodes its declared Rust type.
    pub fn decode(self) -> Result<T> {
        self.source.decode()
    }
}

impl<T: for<'de> DecodeGameValue<'de>> AgentView for LazyView<T> {
    /// Declares the same compact layout as the underlying typed view.
    fn view_schema() -> Schema {
        T::schema()
    }

    /// Keeps the existing payload for decoding on demand.
    fn receive(source: ViewSource) -> Result<Self> {
        Ok(Self {
            source,
            view: PhantomData,
        })
    }
}

/// Owns the existing compact payload from the native or component boundary.
#[doc(hidden)]
pub struct ViewSource(ViewBytes);

enum ViewBytes {
    Shared(Arc<Vec<u8>>),
    Owned(Vec<u8>),
}

impl ViewSource {
    /// Retains a compatible native buffer or converts it to the requested game layout.
    pub fn from_component(value: ComponentValue, schema: &Schema) -> Result<Self> {
        Ok(Self(match value.to_schema_messagepack(schema)? {
            Cow::Borrowed(_) => ViewBytes::Shared(value.bytes),
            Cow::Owned(bytes) => ViewBytes::Owned(bytes),
        }))
    }

    /// Takes ownership of the complete byte list lifted by the component ABI.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_wire(bytes: Vec<u8>) -> Self {
        Self(ViewBytes::Owned(bytes))
    }

    /// Borrows the retained bytes and uses the existing compact decoder.
    fn decode<T: for<'de> DecodeGameValue<'de>>(self) -> Result<T> {
        let bytes = match &self.0 {
            ViewBytes::Shared(bytes) => bytes,
            ViewBytes::Owned(bytes) => bytes,
        };
        decode_declared(bytes).map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps the same layout and defers validation until the policy decodes the view.
    #[test]
    fn deferred_view_uses_the_same_buffer_and_decodes_on_demand() -> Result<()> {
        let schema = <u8 as AgentView>::view_schema();
        assert_eq!(LazyView::<u8>::view_schema(), schema);
        let payload = ComponentValue::from_codec_bytes(crate::encode_compact(&3_u8).unwrap())?;
        let original = payload.as_messagepack().as_ptr();
        let source = ViewSource::from_component(payload, &Schema::Dynamic)?;
        assert!(matches!(&source.0, ViewBytes::Shared(bytes) if bytes.as_ptr() == original));
        assert_eq!(LazyView::<u8>::receive(source)?.decode()?, 3);

        let incompatible = ComponentValue::from_typed("a string")?;
        let source = ViewSource::from_component(incompatible.clone(), &Schema::Dynamic)?;
        assert!(LazyView::<u8>::receive(source)?.decode().is_err());
        let source = ViewSource::from_component(incompatible, &Schema::Dynamic)?;
        assert!(<u8 as AgentView>::receive(source).is_err());
        Ok(())
    }
}
