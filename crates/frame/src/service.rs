use crate::fiber::FiberId;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::Arc;

pub trait Service: Send + Sync + 'static {
    const NAME: &'static str;
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
