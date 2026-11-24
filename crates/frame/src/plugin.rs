use crate::context::Context;
use crate::error::FrameError;
use crate::loader::ConfigValue;
use crate::service::{Service, ServiceKey};
use crate::sync::BoxFuture;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deps {
    keys: Vec<ServiceKey>,
}

impl Deps {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with<S: Service>(mut self) -> Self {
        let key = ServiceKey::of::<S>();
        if !self.keys.iter().any(|k| k.type_id == key.type_id) {
            self.keys.push(key);
        }
        self
    }

    pub fn keys(&self) -> &[ServiceKey] {
        &self.keys
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
