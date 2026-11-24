//! Layer 4 file category, viewer registry contracts, and the shared minimalist
//! white HTML page shell utilities.

pub mod shell;

use frame::{Context, EffectHandle, FrameError, ScopedService, Service};
use http_api::HttpResponse;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

/// High-level semantic classification of a filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileCategory {
    Directory,
    Image,
    Video,
    Audio,
    Code,
    Text,
    Markdown,
    Org,
    Archive,
    Document,
    Executable,
    Unknown,
}

impl FileCategory {
    /// Classify a file from its lowercase extension (without leading dot).
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico"
            | "tiff" | "tif" | "raw" | "heic" | "avif" => Self::Image,
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "m4v"
            | "3gp" | "ogv" | "mpg" | "mpeg" | "m2v" => Self::Video,
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "wma" | "opus"
            | "aiff" | "au" => Self::Audio,
            "rs" | "js" | "mjs" | "cjs" | "ts" | "jsx" | "tsx" | "py"
            | "java" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" | "go"
            | "rb" | "php" | "swift" | "kt" | "scala" | "r" | "m" | "mm"
            | "pl" | "pm" | "t" | "sh" | "bash" | "zsh" | "fish" | "ps1"
            | "bat" | "cmd" | "vbs" | "lua" | "elm" | "erl" | "hrl" | "ex"
            | "exs" | "fs" | "fsx" | "fsi" | "ml" | "mli" | "hs" | "lhs"
            | "clj" | "cljs" | "cljc" | "edn" | "coffee" | "litcoffee"
            | "cr" | "dart" | "groovy" | "gvy" | "gy" | "gsh" | "p6"
            | "pm6" | "pod6" | "t6" | "nim" | "nims" | "zig" | "v" | "vsh" => {
                Self::Code
            }
            "txt" | "rst" | "log" | "csv" | "tsv" | "json" | "xml" | "yaml"
            | "yml" | "toml" | "ini" | "conf" | "cfg" | "properties"
            | "env" | "sql" | "graphql" | "gql" => Self::Text,
            "md" | "markdown" | "mkd" | "mdown" => Self::Markdown,
            "org" => Self::Org,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "lz"
            | "lzma" | "zst" | "br" | "tgz" | "tbz" | "txz" | "tlz" | "cab"
            | "deb" | "rpm" | "dmg" | "pkg" | "msi" | "iso" | "img" => {
                Self::Archive
            }
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "odt" | "ods" | "odp" | "rtf" | "epub" | "mobi" | "azw"
            | "azw3" | "tex" | "latex" => Self::Document,
            "exe" | "dll" | "so" | "dylib" | "bin" | "app" | "elf" | "wasm"
            | "pyc" | "class" | "o" | "obj" | "lib" | "a" => Self::Executable,
            _ => Self::Unknown,
        }
    }

    /// Return a neutral minimalist category label without emoji or color styling.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Directory => "dir",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Code => "code",
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Org => "org",
            Self::Archive => "archive",
            Self::Document => "doc",
            Self::Executable => "bin",
            Self::Unknown => "file",
        }
    }

    /// Return `true` when bare requests render an interactive viewer page by default
    /// (unless web assets like JS scripts or raw media require direct bytes).
    pub const fn is_document_or_code_page(self, ext: &str) -> bool {
        match self {
            Self::Markdown | Self::Org | Self::Text => true,
            Self::Code => !matches_web_script_ext(ext),
            _ => false,
        }
    }
}

const fn matches_web_script_ext(ext: &str) -> bool {
    let b = ext.as_bytes();
    matches!(b, b"js" | b"JS" | b"mjs" | b"MJS" | b"cjs" | b"CJS")
}

/// Requested presentation mode from `?view=...` query or default bare path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    Default,
    Raw,
    Preview,
    Download,
    Viewer,
}

impl ViewMode {
    /// Parse a `?view=` query string value.
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw {
            None => Some(Self::Default),
            Some("raw") => Some(Self::Raw),
            Some("preview") => Some(Self::Preview),
            Some("download") => Some(Self::Download),
            Some("viewer") => Some(Self::Viewer),
            Some(_) => None,
        }
    }
}

/// Read-only context handed to registered [`FileViewer`] implementations.
pub struct ViewerInput<'a> {
    pub rel_path: &'a str,
    pub file_name: &'a str,
    pub full_path: &'a Path,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub category: FileCategory,
    pub view_mode: ViewMode,
    pub mime: &'a str,
    pub allow_hidden: bool,
    pub text: Option<&'a str>,
}

/// Pluggable content viewer (Markdown, code, media, etc.) mounted as an effect.
pub trait FileViewer: Send + Sync + 'static {
    /// Diagnostic name of this viewer plugin.
    fn name(&self) -> &str;

    /// Higher priority viewers are evaluated first when multiple viewers match.
    fn priority(&self) -> i32 {
        0
    }

    /// Return `true` if this viewer can render `(category, mode)`.
    fn supports(&self, category: FileCategory, mode: ViewMode) -> bool;

    /// Produce an HTTP response or return `None` to pass to lower-priority viewers.
    fn render(&self, input: ViewerInput<'_>) -> Option<HttpResponse>;
}

/// Registry contract allowing presentation plugins to register and revoke viewers.
pub trait ViewerRegistry: Send + Sync + 'static {
    /// Register a [`FileViewer`] owned by `caller`, returning an [`EffectHandle`] that revokes it.
    fn register(
        &self,
        caller: &Context,
        viewer: Arc<dyn FileViewer>,
    ) -> Result<EffectHandle, FrameError>;

    /// Evaluate registered viewers in priority order and return the first response.
    fn resolve_response(&self, input: ViewerInput<'_>) -> Option<HttpResponse>;
}

/// Frame service handle wrapping any [`ViewerRegistry`] implementation.
#[derive(Clone)]
pub struct ViewerRegistryService(pub Arc<dyn ViewerRegistry>);

impl ViewerRegistryService {
    /// Wrap a [`ViewerRegistry`] implementation as a registered service handle.
    pub fn new(inner: Arc<dyn ViewerRegistry>) -> Self {
        Self(inner)
    }

    /// Delegate [`ViewerRegistry::register`] with explicit caller ownership.
    pub fn register(
        &self,
        caller: &Context,
        viewer: Arc<dyn FileViewer>,
    ) -> Result<EffectHandle, FrameError> {
        self.0.register(caller, viewer)
    }

    /// Delegate [`ViewerRegistry::resolve_response`].
    pub fn resolve_response(
        &self,
        input: ViewerInput<'_>,
    ) -> Option<HttpResponse> {
        self.0.resolve_response(input)
    }
}

/// Caller-bound [`ViewerRegistryService`] view obtained via `ctx.use_service::<ViewerRegistryService>()`.
#[derive(Clone)]
pub struct ScopedViewerRegistry {
    inner: Arc<ViewerRegistryService>,
    caller: Context,
}

impl ScopedViewerRegistry {
    /// Register a viewer whose lifetime is owned directly by the caller fiber.
    pub fn register(
        &self,
        viewer: Arc<dyn FileViewer>,
    ) -> Result<EffectHandle, FrameError> {
        self.inner.register(&self.caller, viewer)
    }

    /// Evaluate registered viewers in priority order.
    pub fn resolve_response(
        &self,
        input: ViewerInput<'_>,
    ) -> Option<HttpResponse> {
        self.inner.resolve_response(input)
    }
}

impl Service for ViewerRegistryService {
    const NAME: &'static str = "files.viewer_registry";
}

impl ScopedService for ViewerRegistryService {
    type View = ScopedViewerRegistry;

    fn bind(self: Arc<Self>, caller: Context) -> Self::View {
        ScopedViewerRegistry {
            inner: self,
            caller,
        }
    }
}
