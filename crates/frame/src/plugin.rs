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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFieldType {
    Bool,
    Int,
    Float,
    Str,
    List,
    Map,
    Any,
}

impl ConfigFieldType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Str => "string",
            Self::List => "list",
            Self::Map => "map",
            Self::Any => "any",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigFieldSchema {
    pub name: String,
    pub kind: ConfigFieldType,
    pub required: bool,
    pub nullable: bool,
    pub sensitive: bool,
    pub default: Option<ConfigValue>,
    pub description: String,
}

impl ConfigFieldSchema {
    pub fn new(
        name: impl Into<String>,
        kind: ConfigFieldType,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
            nullable: false,
            sensitive: false,
            default: None,
            description: description.into(),
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    pub fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }

    pub fn with_default(mut self, default: ConfigValue) -> Self {
        self.default = Some(default);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginSchema {
    pub description: String,
    pub fields: Vec<ConfigFieldSchema>,
}

impl PluginSchema {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            fields: Vec::new(),
        }
    }

    pub fn field(mut self, field: ConfigFieldSchema) -> Self {
        self.fields.push(field);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginDescriptor {
    pub name: String,
    pub description: String,
    pub required_deps: Vec<&'static str>,
    pub optional_deps: Vec<&'static str>,
    pub schema: PluginSchema,
}

pub trait Plugin: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn deps(&self) -> Deps {
        Deps::empty()
    }

    fn schema(&self) -> PluginSchema {
        PluginSchema::default()
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
