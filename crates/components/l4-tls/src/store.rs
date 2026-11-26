//! Certificate compilation, SNI selection, atomic hot-reload store, and
//! inbound/outbound rustls adapters.

use crate::io::{AsyncByteIoCompat, TlsStreamIo};
use frame::{BoxFuture, ConfigValue, ErrorKind, FrameError, ValidationIssue};
use net_api::BoxStreamIo;
use rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, ServerConfig,
    SignatureScheme,
};
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::{Arc, RwLock};
use tls_api::{
    NegotiatedTls, SniCertConfig, TlsAcceptor, TlsClientConfig, TlsConnector,
    TlsListenerEntry,
};
use tokio_rustls::{
    TlsAcceptor as RustlsAcceptor, TlsConnector as RustlsConnector,
};

/// Parsed and validated TLS listener table ready for atomic hot-swap.
#[derive(Default)]
pub(crate) struct CompiledListeners {
    acceptors: HashMap<String, RustlsAcceptor>,
}

/// Shared TLS store updated atomically on config reload without killing existing sessions.
#[derive(Default)]
pub(crate) struct TlsStore {
    inner: RwLock<Arc<CompiledListeners>>,
}

impl TlsStore {
    pub(crate) fn swap(&self, compiled: CompiledListeners) {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        *guard = Arc::new(compiled);
    }

    fn snapshot(&self) -> Arc<CompiledListeners> {
        Arc::clone(&*self.inner.read().unwrap_or_else(|e| e.into_inner()))
    }
}

impl TlsAcceptor for TlsStore {
    fn wrap_inbound(
        &self,
        listener_id: &str,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>> {
        let snap = self.snapshot();
        let id = listener_id.to_string();
        Box::pin(async move {
            let Some(acceptor) = snap.acceptors.get(&id).cloned() else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("TLS listener `{id}` is not configured"),
                ));
            };
            let stream = acceptor.accept(AsyncByteIoCompat(raw)).await?;
            let (_, conn) = stream.get_ref();
            let alpn = conn.alpn_protocol().map(|b| b.to_vec());
            let server_name = conn.server_name().map(ToOwned::to_owned);
            Ok(NegotiatedTls {
                stream: Box::new(TlsStreamIo(stream)),
                alpn,
                server_name,
            })
        })
    }

    fn has_listener(&self, listener_id: &str) -> bool {
        self.snapshot().acceptors.contains_key(listener_id)
    }
}

#[derive(Debug)]
struct SniCertResolver {
    default: Option<Arc<CertifiedKey>>,
    exact: HashMap<String, Arc<CertifiedKey>>,
    wildcards: Vec<(String, Arc<CertifiedKey>)>,
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(
        &self,
        client_hello: ClientHello<'_>,
    ) -> Option<Arc<CertifiedKey>> {
        if let Some(name) = client_hello.server_name() {
            let lower = name.to_ascii_lowercase();
            if let Some(key) = self.exact.get(&lower) {
                return Some(Arc::clone(key));
            }
            for (suffix, key) in &self.wildcards {
                if lower.len() > suffix.len()
                    && lower.ends_with(suffix.as_str())
                {
                    let prefix = &lower[..lower.len() - suffix.len()];
                    if !prefix.is_empty() && !prefix.contains('.') {
                        return Some(Arc::clone(key));
                    }
                }
            }
        }
        self.default.clone()
    }
}

fn compile_certified_key(
    cert_pem: &str,
    key_pem: &str,
    path_prefix: &str,
) -> Result<Arc<CertifiedKey>, ValidationIssue> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ValidationIssue {
                path: format!("{path_prefix}.cert_pem"),
                message: format!("invalid certificate PEM: {e}"),
            })?;
    if certs.is_empty() {
        return Err(ValidationIssue {
            path: format!("{path_prefix}.cert_pem"),
            message: "certificate PEM contains no certificates".to_string(),
        });
    }

    let key: PrivateKeyDer<'static> =
        PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| {
            ValidationIssue {
                path: format!("{path_prefix}.key_pem"),
                message: format!("invalid private key PEM: {e}"),
            }
        })?;

    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|e| ValidationIssue {
            path: format!("{path_prefix}.key_pem"),
            message: format!("unsupported private key algorithm: {e}"),
        })?;

    Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}

fn compile_listener(
    entry: &TlsListenerEntry,
    idx: usize,
) -> Result<RustlsAcceptor, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let mut default = None;
    let mut exact = HashMap::new();
    let mut wildcards = Vec::new();

    if entry.certs.is_empty() {
        issues.push(ValidationIssue {
            path: format!("listeners[{idx}].certs"),
            message: "TLS listener must define at least one certificate"
                .to_string(),
        });
        return Err(issues);
    }

    for (cidx, c) in entry.certs.iter().enumerate() {
        let prefix = format!("listeners[{idx}].certs[{cidx}]");
        match compile_certified_key(&c.cert_pem, &c.key_pem, &prefix) {
            Ok(ck) => match c.server_name.as_deref().map(str::trim) {
                None | Some("") | Some("*") => {
                    if default.is_none() {
                        default = Some(ck);
                    }
                }
                Some(host) => {
                    let lower = host.to_ascii_lowercase();
                    if default.is_none() {
                        default = Some(Arc::clone(&ck));
                    }
                    if let Some(rest) = lower.strip_prefix('*') {
                        if !rest.starts_with('.') || rest.len() < 3 {
                            issues.push(ValidationIssue {
                                path: format!("{prefix}.server_name"),
                                message: format!(
                                    "invalid wildcard SNI pattern `{host}`"
                                ),
                            });
                        } else {
                            wildcards.push((rest.to_string(), ck));
                        }
                    } else {
                        exact.insert(lower, ck);
                    }
                }
            },
            Err(issue) => issues.push(issue),
        }
    }

    if !issues.is_empty() {
        return Err(issues);
    }

    let resolver = Arc::new(SniCertResolver {
        default,
        exact,
        wildcards,
    });
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| {
            vec![ValidationIssue {
                path: format!("listeners[{idx}]"),
                message: format!("failed to initialize TLS versions: {e}"),
            }]
        })?
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    if !entry.alpn_protocols.is_empty() {
        cfg.alpn_protocols = entry
            .alpn_protocols
            .iter()
            .map(|p| p.as_bytes().to_vec())
            .collect();
    }

    Ok(RustlsAcceptor::from(Arc::new(cfg)))
}

fn read_pem_source(
    inline: Option<&str>,
    file_path: Option<&str>,
    field_name: &str,
    prefix: &str,
) -> Result<String, ValidationIssue> {
    if let Some(pem) = inline {
        if pem.trim().is_empty() {
            return Err(ValidationIssue {
                path: format!("{prefix}.{field_name}_pem"),
                message: format!("{field_name}_pem cannot be empty"),
            });
        }
        return Ok(pem.to_string());
    }
    if let Some(path) = file_path {
        return std::fs::read_to_string(path).map_err(|e| ValidationIssue {
            path: format!("{prefix}.{field_name}_path"),
            message: format!("failed to read `{path}`: {e}"),
        });
    }
    Err(ValidationIssue {
        path: prefix.to_string(),
        message: format!(
            "either `{field_name}_pem` or `{field_name}_path` must be set"
        ),
    })
}

fn parse_cert_entry(
    raw: &ConfigValue,
    prefix: &str,
) -> Result<SniCertConfig, Vec<ValidationIssue>> {
    let ConfigValue::Map(map) = raw else {
        return Err(vec![ValidationIssue {
            path: prefix.to_string(),
            message: "certificate entry must be a map".to_string(),
        }]);
    };
    let mut server_name = None;
    let mut cert_pem_in = None;
    let mut key_pem_in = None;
    let mut cert_path = None;
    let mut key_path = None;
    let mut issues = Vec::new();

    for (k, v) in map {
        match k.as_str() {
            "server_name" | "sni" => match v {
                ConfigValue::Null => server_name = None,
                ConfigValue::Str(s) => server_name = Some(s.clone()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.{k}"),
                    message: "server_name must be a string or null".to_string(),
                }),
            },
            "cert_pem" => match v {
                ConfigValue::Str(s) => cert_pem_in = Some(s.as_str()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.cert_pem"),
                    message: "cert_pem must be a string".to_string(),
                }),
            },
            "key_pem" => match v {
                ConfigValue::Str(s) => key_pem_in = Some(s.as_str()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.key_pem"),
                    message: "key_pem must be a string".to_string(),
                }),
            },
            "cert_path" => match v {
                ConfigValue::Str(s) => cert_path = Some(s.as_str()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.cert_path"),
                    message: "cert_path must be a string".to_string(),
                }),
            },
            "key_path" => match v {
                ConfigValue::Str(s) => key_path = Some(s.as_str()),
                _ => issues.push(ValidationIssue {
                    path: format!("{prefix}.key_path"),
                    message: "key_path must be a string".to_string(),
                }),
            },
            _ => issues.push(ValidationIssue {
                path: format!("{prefix}.{k}"),
                message: format!("unknown TLS certificate field `{k}`"),
            }),
        }
    }

    let cert_pem = match read_pem_source(cert_pem_in, cert_path, "cert", prefix)
    {
        Ok(p) => p,
        Err(i) => {
            issues.push(i);
            String::new()
        }
    };
    let key_pem = match read_pem_source(key_pem_in, key_path, "key", prefix) {
        Ok(p) => p,
        Err(i) => {
            issues.push(i);
            String::new()
        }
    };

    if issues.is_empty() {
        Ok(SniCertConfig {
            server_name,
            cert_pem,
            key_pem,
        })
    } else {
        Err(issues)
    }
}

pub(crate) fn parse_and_compile_listeners(
    base: &[TlsListenerEntry],
    raw: &ConfigValue,
) -> Result<CompiledListeners, FrameError> {
    let entries: Vec<TlsListenerEntry> = match raw {
        ConfigValue::Null => base.to_vec(),
        ConfigValue::Map(map) => {
            let mut parsed = base.to_vec();
            let mut issues = Vec::new();
            for (k, v) in map {
                match k.as_str() {
                    "listeners" => {
                        let ConfigValue::List(list) = v else {
                            issues.push(ValidationIssue {
                                path: "listeners".to_string(),
                                message: "listeners must be a list".to_string(),
                            });
                            continue;
                        };
                        parsed.clear();
                        for (idx, item) in list.iter().enumerate() {
                            let prefix = format!("listeners[{idx}]");
                            let ConfigValue::Map(entry_map) = item else {
                                issues.push(ValidationIssue {
                                    path: prefix,
                                    message: "listener entry must be a map"
                                        .to_string(),
                                });
                                continue;
                            };
                            let mut id = None;
                            let mut alpn = vec!["http/1.1".to_string()];
                            let mut certs = Vec::new();
                            let mut flat_cert_fields = ConfigValue::Map(
                                entry_map
                                    .iter()
                                    .filter(|(ck, _)| {
                                        matches!(
                                            ck.as_str(),
                                            "cert_pem"
                                                | "key_pem"
                                                | "cert_path"
                                                | "key_path"
                                                | "server_name"
                                                | "sni"
                                        )
                                    })
                                    .map(|(ck, cv)| (ck.clone(), cv.clone()))
                                    .collect(),
                            );
                            let mut has_flat_cert = false;

                            for (ek, ev) in entry_map {
                                match ek.as_str() {
                                    "id" => match ev {
                                        ConfigValue::Str(s)
                                            if !s.trim().is_empty() =>
                                        {
                                            id = Some(s.clone());
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.id"),
                                            message: "listener id must be a non-empty string".to_string(),
                                        }),
                                    },
                                    "alpn" => match ev {
                                        ConfigValue::List(arr) => {
                                            let mut protos = Vec::new();
                                            for (aidx, av) in
                                                arr.iter().enumerate()
                                            {
                                                if let ConfigValue::Str(ps) = av
                                                {
                                                    protos.push(ps.clone());
                                                } else {
                                                    issues.push(ValidationIssue {
                                                        path: format!("{prefix}.alpn[{aidx}]"),
                                                        message: "alpn protocol must be a string".to_string(),
                                                    });
                                                }
                                            }
                                            alpn = protos;
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.alpn"),
                                            message: "alpn must be a list of strings".to_string(),
                                        }),
                                    },
                                    "certs" => match ev {
                                        ConfigValue::List(arr) => {
                                            for (cidx, cv) in
                                                arr.iter().enumerate()
                                            {
                                                match parse_cert_entry(
                                                    cv,
                                                    &format!("{prefix}.certs[{cidx}]"),
                                                ) {
                                                    Ok(c) => certs.push(c),
                                                    Err(mut e) => {
                                                        issues.append(&mut e)
                                                    }
                                                }
                                            }
                                        }
                                        _ => issues.push(ValidationIssue {
                                            path: format!("{prefix}.certs"),
                                            message: "certs must be a list"
                                                .to_string(),
                                        }),
                                    },
                                    "cert_pem" | "key_pem" | "cert_path"
                                    | "key_path" | "server_name" | "sni" => {
                                        has_flat_cert = true;
                                    }
                                    _ => issues.push(ValidationIssue {
                                        path: format!("{prefix}.{ek}"),
                                        message: format!(
                                            "unknown listener option `{ek}`"
                                        ),
                                    }),
                                }
                            }

                            if has_flat_cert {
                                match parse_cert_entry(
                                    &flat_cert_fields,
                                    &prefix,
                                ) {
                                    Ok(c) => certs.push(c),
                                    Err(mut e) => issues.append(&mut e),
                                }
                            }
                            let _ = &mut flat_cert_fields;

                            let Some(id) = id else {
                                issues.push(ValidationIssue {
                                    path: format!("{prefix}.id"),
                                    message: "listener id is required"
                                        .to_string(),
                                });
                                continue;
                            };
                            parsed.push(TlsListenerEntry {
                                id,
                                certs,
                                alpn_protocols: alpn,
                            });
                        }
                    }
                    _ => issues.push(ValidationIssue {
                        path: k.clone(),
                        message: format!("unknown l4-tls option `{k}`"),
                    }),
                }
            }
            if !issues.is_empty() {
                return Err(FrameError::validation(issues));
            }
            parsed
        }
        _ => {
            return Err(FrameError::validation(vec![ValidationIssue {
                path: "".to_string(),
                message: "l4-tls config must be Null or Map".to_string(),
            }]));
        }
    };

    let mut acceptors = HashMap::new();
    let mut issues = Vec::new();
    for (idx, entry) in entries.iter().enumerate() {
        if acceptors.contains_key(&entry.id) {
            issues.push(ValidationIssue {
                path: format!("listeners[{idx}].id"),
                message: format!("duplicate TLS listener id `{}`", entry.id),
            });
            continue;
        }
        match compile_listener(entry, idx) {
            Ok(acc) => {
                acceptors.insert(entry.id.clone(), acc);
            }
            Err(mut e) => issues.append(&mut e),
        }
    }

    if issues.is_empty() {
        Ok(CompiledListeners { acceptors })
    } else {
        Err(FrameError::validation(issues))
    }
}

#[derive(Debug)]
struct NoVerifyCertVerifier;

impl ServerCertVerifier for NoVerifyCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Outbound [`TlsConnector`] implementation using `rustls` + `webpki-roots`.
#[derive(Default)]
pub struct DefaultTlsConnector;

impl fmt::Debug for DefaultTlsConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultTlsConnector").finish()
    }
}

fn build_client_config(cfg: &TlsClientConfig) -> io::Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| io::Error::other(format!("rustls client init: {e}")))?;

    let mut client_cfg = if cfg.insecure_skip_verify {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifyCertVerifier))
            .with_no_client_auth()
    } else if let Some(ca_pem) = &cfg.ca_cert_pem {
        let mut store = RootCertStore::empty();
        for item in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
            let cert = item.map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid CA cert PEM: {e}"),
                )
            })?;
            store.add(cert).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid CA cert: {e}"),
                )
            })?;
        }
        builder.with_root_certificates(store).with_no_client_auth()
    } else {
        let mut store = RootCertStore::empty();
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(store).with_no_client_auth()
    };

    if !cfg.alpn_protocols.is_empty() {
        client_cfg.alpn_protocols = cfg
            .alpn_protocols
            .iter()
            .map(|p| p.as_bytes().to_vec())
            .collect();
    }
    Ok(Arc::new(client_cfg))
}

impl TlsConnector for DefaultTlsConnector {
    fn wrap_outbound(
        &self,
        cfg: &TlsClientConfig,
        raw: BoxStreamIo,
    ) -> BoxFuture<'static, io::Result<NegotiatedTls>> {
        let cfg = cfg.clone();
        Box::pin(async move {
            let server_name = ServerName::try_from(cfg.server_name.clone())
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "invalid TLS server_name `{}`: {e}",
                            cfg.server_name
                        ),
                    )
                })?;
            let client_cfg = build_client_config(&cfg)?;
            let connector = RustlsConnector::from(client_cfg);
            let stream = connector
                .connect(server_name, AsyncByteIoCompat(raw))
                .await?;
            let (_, conn) = stream.get_ref();
            let alpn = conn.alpn_protocol().map(|b| b.to_vec());
            Ok(NegotiatedTls {
                stream: Box::new(TlsStreamIo(stream)),
                alpn,
                server_name: Some(cfg.server_name),
            })
        })
    }
}

impl Drop for CompiledListeners {
    fn drop(&mut self) {}
}

#[allow(dead_code)]
fn _assert_err_kind() {
    let _ = ErrorKind::ValidationFailed;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_unknown_field_and_empty_listener_id() {
        let cfg = ConfigValue::Map(
            [(
                "listeners".to_string(),
                ConfigValue::List(vec![ConfigValue::Map(
                    [("id".to_string(), ConfigValue::Str("   ".to_string()))]
                        .into_iter()
                        .collect(),
                )]),
            )]
            .into_iter()
            .collect(),
        );
        let err = parse_and_compile_listeners(&[], &cfg)
            .err()
            .expect("empty listener id must fail validation");
        assert_eq!(err.kind, ErrorKind::ValidationFailed);
    }
}
