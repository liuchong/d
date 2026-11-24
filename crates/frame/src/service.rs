use crate::context::Context;
use crate::fiber::FiberId;
use crate::loader::ConfigValue;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

pub trait Service: Send + Sync + 'static {
    const NAME: &'static str;
}

/// Service exposing a caller-bound view whose registered effects belong to
/// the calling plugin's [`Context`] rather than the provider's.
pub trait ScopedService: Service {
    type View;

    fn bind(self: Arc<Self>, caller: Context) -> Self::View;
}

/// Deep-merge `patch` onto `base` for [`ConfigValue::Map`], replacing non-map variants.
pub fn merge_config_values(
    base: ConfigValue,
    patch: ConfigValue,
) -> ConfigValue {
    match (base, patch) {
        (ConfigValue::Map(mut base_map), ConfigValue::Map(patch_map)) => {
            for (k, v) in patch_map {
                match base_map.remove(&k) {
                    Some(existing) => {
                        base_map.insert(k, merge_config_values(existing, v));
                    }
                    None => {
                        base_map.insert(k, v);
                    }
                }
            }
            ConfigValue::Map(base_map)
        }
        (_, patch) => patch,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServiceKey {
    pub type_id: TypeId,
    pub name: &'static str,
}

impl ServiceKey {
    pub fn of<S: Service>() -> Self {
        Self {
            type_id: TypeId::of::<S>(),
            name: S::NAME,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeToken(pub u64);

impl ScopeToken {
    pub const ROOT: Self = Self(0);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    overrides: BTreeMap<TypeId, ScopeToken>,
}

impl Scope {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn token_for(&self, type_id: TypeId) -> ScopeToken {
        self.overrides
            .get(&type_id)
            .copied()
            .unwrap_or(ScopeToken::ROOT)
    }

    pub fn with_isolated(&self, type_id: TypeId, token: ScopeToken) -> Self {
        let mut next = self.clone();
        next.overrides.insert(type_id, token);
        next
    }
}

pub type ReadinessCheck<S> = Arc<dyn Fn(&S) -> bool + Send + Sync>;

pub(crate) type ErasedReadinessCheck =
    Arc<dyn Fn(&(dyn Any + Send + Sync)) -> bool + Send + Sync>;

pub(crate) struct ServiceRecord {
    pub(crate) key: ServiceKey,
    pub(crate) scope: ScopeToken,
    pub(crate) owner: FiberId,
    pub(crate) instance_seq: u64,
    pub(crate) value: Arc<dyn Any + Send + Sync>,
    pub(crate) check: Option<ErasedReadinessCheck>,
}

impl ServiceRecord {
    pub(crate) fn is_ready(&self) -> bool {
        match &self.check {
            Some(check) => check(self.value.as_ref()),
            None => true,
        }
    }

    pub(crate) fn downcast_arc<S: Service>(&self) -> Option<Arc<S>> {
        Arc::clone(&self.value).downcast::<S>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_config_values_deep_merges_maps_and_replaces_scalars() {
        let mut base = BTreeMap::new();
        base.insert("a".to_string(), ConfigValue::Int(1));
        let mut nested = BTreeMap::new();
        nested.insert("x".to_string(), ConfigValue::Bool(true));
        nested.insert("y".to_string(), ConfigValue::Int(10));
        base.insert("nested".to_string(), ConfigValue::Map(nested));

        let mut patch = BTreeMap::new();
        let mut nested_patch = BTreeMap::new();
        nested_patch.insert("y".to_string(), ConfigValue::Int(20));
        patch.insert("nested".to_string(), ConfigValue::Map(nested_patch));
        patch.insert("b".to_string(), ConfigValue::Str("ok".into()));

        let merged = merge_config_values(
            ConfigValue::Map(base),
            ConfigValue::Map(patch),
        );
        let ConfigValue::Map(out) = merged else {
            panic!("expected map");
        };
        assert_eq!(out.get("a"), Some(&ConfigValue::Int(1)));
        assert_eq!(out.get("b"), Some(&ConfigValue::Str("ok".into())));
        let Some(ConfigValue::Map(nested_out)) = out.get("nested") else {
            panic!("expected nested map");
        };
        assert_eq!(nested_out.get("x"), Some(&ConfigValue::Bool(true)));
        assert_eq!(nested_out.get("y"), Some(&ConfigValue::Int(20)));
        assert_eq!(
            merge_config_values(ConfigValue::Int(1), ConfigValue::Bool(false)),
            ConfigValue::Bool(false)
        );
    }
}
