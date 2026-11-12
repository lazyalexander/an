use crate::backend::BackendKind;
use crate::identify::Selection;
use crate::state::{Document, Outcome, RenderRequest};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;
use tiny_http::{Header, Method, Response, Server, StatusCode};

pub fn serve(root: &Path) -> io::Result<()> {
    let server =
        Server::http("127.0.0.1:0").map_err(|error| io::Error::other(error.to_string()))?;
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(address) => address.port(),
        other => {
            return Err(io::Error::other(format!(
                "refusing non-loopback listener {other:?}"
            )))
        }
    };
    fs::create_dir_all(root.join("state"))?;
    fs::write(root.join("state/port"), port.to_string())?;
    let document = Mutex::new(Document::open(root).map_err(|error| io::Error::other(error))?);
    for mut request in server.incoming_requests() {
        let response = route(&document, &mut request);
        let _ = request.respond(response);
    }
    Ok(())
}

fn route(
    document: &Mutex<Document>,
    request: &mut tiny_http::Request,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let path = request.url().split('?').next().unwrap_or("");
    let outcome = match (request.method(), path) {
        (&Method::Get, "/v1/health") => {
            let doc = lock_doc(document);
            json!({"revision": doc.revision(), "locked": doc.locked()})
        }
        (&Method::Post, "/v1/render") => render(document, request),
        (&Method::Post, "/v1/commit") => finish(document, request, true),
        (&Method::Post, "/v1/abort") => finish(document, request, false),
        _ => json!({"status":"failed","reason":"backend","detail":"unknown route"}),
    };
    json_response(outcome)
}

fn render(document: &Mutex<Document>, request: &mut tiny_http::Request) -> Value {
    let body = match read_body(request) {
        Ok(body) => body,
        Err(error) => return failed(&error),
    };
    let request: RenderBody = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return failed(&error.to_string()),
    };
    let Ok(strokes) = STANDARD.decode(request.png_base64) else {
        return failed("png_base64 is not base64");
    };
    let Ok(backend) = parse_backend(&request.backend) else {
        return failed("backend must be code, dsh, or codex");
    };
    let mut doc = lock_doc(document);
    let outcome = doc.render(&RenderRequest {
        revision: request.revision,
        backend,
        description: request.description,
        strokes_png: strokes,
        selection: request.selection,
    });
    match outcome {
        Outcome::Ready {
            attempt,
            revision,
            recognized,
            layers,
        } => json!({
            "status": "ready",
            "attempt": attempt,
            "revision": revision,
            "recognized": recognized,
            "layers": layers.into_iter().map(|layer| json!({
                "name": layer.name,
                "png_base64": STANDARD.encode(layer.png),
            })).collect::<Vec<_>>(),
        }),
        other => {
            serde_json::to_value(other).unwrap_or_else(|_| failed("could not encode the outcome"))
        }
    }
}

fn finish(document: &Mutex<Document>, request: &mut tiny_http::Request, commit: bool) -> Value {
    let body = match read_body(request) {
        Ok(body) => body,
        Err(error) => return failed(&error),
    };
    let attempt = serde_json::from_slice::<AttemptBody>(&body)
        .map(|body| body.attempt)
        .unwrap_or_default();
    let mut doc = lock_doc(document);
    if commit {
        match doc.commit(&attempt) {
            Ok(revision) => json!({"revision": revision}),
            Err(error) => failed(&format!("{error:?}")),
        }
    } else {
        match doc.abort(&attempt) {
            Ok(()) => json!({"revision": doc.revision()}),
            Err(error) => failed(&format!("{error:?}")),
        }
    }
}

fn read_body(request: &mut tiny_http::Request) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() > 8 * 1024 * 1024 {
        return Err("request is larger than 8 MiB".into());
    }
    Ok(body)
}

fn json_response(value: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let bytes = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    let header =
        Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    Response::from_data(bytes)
        .with_status_code(StatusCode(200))
        .with_header(header)
}

fn failed(detail: &str) -> Value {
    json!({"status":"failed","reason":"backend","detail": detail})
}

fn lock_doc(document: &Mutex<Document>) -> std::sync::MutexGuard<'_, Document> {
    document.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn parse_backend(value: &str) -> Result<BackendKind, ()> {
    match value {
        "code" => Ok(BackendKind::Code),
        "dsh" => Ok(BackendKind::Dsh),
        "codex" => Ok(BackendKind::Codex),
        _ => Err(()),
    }
}

#[derive(Deserialize)]
struct RenderBody {
    revision: u64,
    backend: String,
    description: String,
    png_base64: String,
    selection: Option<Selection>,
}

#[derive(Deserialize)]
struct AttemptBody {
    attempt: String,
}
