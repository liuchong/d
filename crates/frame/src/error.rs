use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InactiveFiber,
    DuplicateService,
    MissingDependency,
    NotServiceOwner,
    ValidationFailed,
    UnknownEntry,
    DuplicateEntryId,
    PluginNotFound,
    LoaderClosed,
    ActivationFailed,
    CleanupFailed,
    Aggregate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct FrameError {
    pub kind: ErrorKind,
    pub message: String,
    pub issues: Vec<ValidationIssue>,
    pub causes: Vec<FrameError>,
}

impl FrameError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            issues: Vec::new(),
            causes: Vec::new(),
        }
    }

    pub fn validation(issues: Vec<ValidationIssue>) -> Self {
        Self {
            kind: ErrorKind::ValidationFailed,
            message: "configuration validation failed".to_string(),
            issues,
            causes: Vec::new(),
        }
    }

    pub fn aggregate(
        message: impl Into<String>,
        causes: Vec<FrameError>,
    ) -> Self {
        Self {
            kind: ErrorKind::Aggregate,
            message: message.into(),
            issues: Vec::new(),
            causes,
        }
    }

    pub fn inactive_fiber() -> Self {
        Self::new(ErrorKind::InactiveFiber, "fiber is inactive or disposed")
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for FrameError {}
