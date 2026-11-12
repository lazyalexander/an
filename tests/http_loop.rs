use an::render::encode;
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{Rgba, RgbaImage};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[test]
fn the_code_backend_commits_two_layers_and_repaints_one() {
    let root = temp("loop");
    let port = start(&root);
    let png = two_blobs();
    let first = post(
        port,
        "/v1/render",
        &json!({
            "document": "default",
            "revision": 0,
            "backend": "code",
            "description": "红色轮子，黄铜握把",
            "png_base64": STANDARD.encode(&png),
            "selection": null
        }),
    );
    assert_eq!(first["status"], "ready");
    assert_eq!(first["revision"], 0);
    assert_eq!(first["layers"].as_array().unwrap().len(), 2);
    assert_eq!(first["layers"][0]["name"], "an/轮子");
    assert_eq!(first["layers"][1]["name"], "an/握把");

    let locked = post(
        port,
        "/v1/render",
        &json!({
            "document": "default", "revision": 0, "backend": "code",
            "description": "红色轮子，黄铜握把", "png_base64": STANDARD.encode(&png), "selection": null
        }),
    );
    assert_eq!(locked["status"], "rejected");
    assert_eq!(locked["reason"], "locked");

    post(port, "/v1/commit", &json!({"attempt": first["attempt"]}));
    let health = get(port, "/v1/health");
    assert_eq!(health["revision"], 1);
    assert_eq!(health["locked"], false);

    let stale = post(
        port,
        "/v1/render",
        &json!({
            "document": "default", "revision": 0, "backend": "code",
            "description": "红色轮子，黄铜握把", "png_base64": STANDARD.encode(&png), "selection": null
        }),
    );
    assert_eq!(stale["reason"], "stale");

    let right = fs::read(layer(&root, "an/握把")).unwrap();
    let edit = post(
        port,
        "/v1/render",
        &json!({
            "document": "default", "revision": 1, "backend": "code",
            "description": "蓝色轮子", "png_base64": STANDARD.encode(&png),
            "selection": {"x": 0, "y": 0, "w": 12, "h": 8}
        }),
    );
    assert_eq!(edit["recognized"], "轮子");
    post(port, "/v1/commit", &json!({"attempt": edit["attempt"]}));
    assert_eq!(fs::read(layer(&root, "an/握把")).unwrap(), right);
}

#[test]
fn a_failing_dsh_does_not_lock_or_fall_back() {
    let root = temp("dsh");
    let program = root.join("fail.sh");
    fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&program).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&program, permissions).unwrap();
    }
    fs::write(root.join("backends.toml"), format!("[dsh]\nargv = [\"{}\"]\ntimeout_secs = 2\n[codex]\nargv = [\"{}\"]\ntimeout_secs = 2\n", program.display(), program.display())).unwrap();
    let port = start(&root);
    let response = post(
        port,
        "/v1/render",
        &json!({
            "document": "default", "revision": 0, "backend": "dsh",
            "description": "红色轮子", "png_base64": STANDARD.encode(two_blobs()), "selection": null
        }),
    );
    assert_eq!(response["status"], "failed");
    assert_eq!(get(port, "/v1/health")["revision"], 0);
    assert_eq!(get(port, "/v1/health")["locked"], false);
}

fn two_blobs() -> Vec<u8> {
    let mut image = RgbaImage::new(40, 8);
    for x in 0..8 {
        image.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
        image.put_pixel(x + 20, 0, Rgba([0, 0, 0, 255]));
    }
    encode(&image).unwrap()
}

fn start(root: &Path) -> u16 {
    let root = root.to_path_buf();
    let port_file = root.join("state/port");
    thread::spawn(move || an::serve(&root).unwrap());
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&port_file) {
            if let Ok(port) = text.trim().parse() {
                return port;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("server did not write a port");
}

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("an-http-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn layer(root: &Path, name: &str) -> PathBuf {
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("state/default/layers/manifest.json")).unwrap())
            .unwrap();
    let file = manifest
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap()["file"]
        .as_str()
        .unwrap();
    root.join("state/default/layers").join(file)
}

fn post(port: u16, path: &str, body: &Value) -> Value {
    exchange(port, "POST", path, &body.to_string())
}

fn get(port: u16, path: &str) -> Value {
    exchange(port, "GET", path, "")
}

fn exchange(port: u16, method: &str, path: &str, body: &str) -> Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(request.as_bytes()).unwrap();
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).unwrap();
    let text = String::from_utf8(buffer).unwrap();
    let json = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(json).unwrap_or_else(|_| panic!("response was {text}"))
}
