use crate::context::Context;
use crate::error::FrameError;
use crate::loader::ConfigValue;
use crate::service::{Service, ServiceKey};
use crate::sync::BoxFuture;
use std::any::TypeId;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deps {
    required: Vec<ServiceKey>,
    optional: Vec<ServiceKey>,
}

impl Deps {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Declare a required dependency on `S`. Upgrades `S` if previously optional.
    pub fn with<S: Service>(mut self) -> Self {
        let key = ServiceKey::of::<S>();
        self.optional.retain(|k| k.type_id != key.type_id);
        if !self.required.iter().any(|k| k.type_id == key.type_id) {
            self.required.push(key);
        }
        self
    }

    /// Declare an optional dependency on `S`. Missing optional services never
    /// block activation, while additions/removals/replacements trigger reload.
    pub fn with_optional<S: Service>(mut self) -> Self {
        let key = ServiceKey::of::<S>();
        if !self.required.iter().any(|k| k.type_id == key.type_id)
            && !self.optional.iter().any(|k| k.type_id == key.type_id)
        {
            self.optional.push(key);
        }
        self
    }

    /// Required service keys.
    pub fn keys(&self) -> &[ServiceKey] {
        &self.required
    }

    /// Optional service keys.
    pub fn optional_keys(&self) -> &[ServiceKey] {
        &self.optional
    }

    /// Return `true` if `tid` is registered as either required or optional.
    pub fn contains_type(&self, tid: TypeId) -> bool {
        self.required.iter().any(|k| k.type_id == tid)
            || self.optional.iter().any(|k| k.type_id == tid)
    }
}

pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn validate_config(
        &self,
        raw: &ConfigValue,
    ) -> Result<ConfigValue, FrameError> {
        Ok(raw.clone())
    }

    fn apply(
        &self,
        ctx: Context,
        config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>>;
}

pub(crate) struct FnPlugin<F> {
    pub(crate) name: String,
    pub(crate) deps: Deps,
    pub(crate) apply: Arc<F>,
}

impl<F> Plugin for FnPlugin<F>
where
    F: Fn(Context) -> BoxFuture<'static, Result<(), FrameError>>
        + Send
        + Sync
        + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn deps(&self) -> Deps {
        self.deps.clone()
    }

    fn apply(
        &self,
        ctx: Context,
        _config: ConfigValue,
    ) -> BoxFuture<'static, Result<(), FrameError>> {
        (self.apply)(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SvcOne;
    impl Service for SvcOne {
        const NAME: &'static str = "one";
    }
    struct SvcTwo;
    impl Service for SvcTwo {
        const NAME: &'static str = "two";
    }

    #[test]
    fn deps_dedupes_and_promotes_optional_to_required() {
        let deps = Deps::empty()
            .with_optional::<SvcOne>()
            .with_optional::<SvcOne>()
            .with::<SvcOne>()
            .with_optional::<SvcTwo>();
        assert_eq!(deps.keys().len(), 1);
        assert_eq!(deps.keys()[0].name, "one");
        assert_eq!(deps.optional_keys().len(), 1);
        assert_eq!(deps.optional_keys()[0].name, "two");
        assert!(deps.contains_type(TypeId::of::<SvcOne>()));
        assert!(deps.contains_type(TypeId::of::<SvcTwo>()));
    }
}
