use anyhow::{ensure, Context, Result};
use std::io::{Cursor, Read};

pub static ADAPTER: super::Adapter = super::Adapter {
    id: "krita",
    inputs: &["kra", "ora"],
    snapshot: "submitted.kra",
    output: "painting.kra",
    verify: verify_snapshot,
};

fn verify_document_xml(data: &[u8]) -> Result<()> {
    use quick_xml::{events::Event, Reader};
    let mut reader = Reader::from_reader(data);
    let mut depth = 0usize;
    let mut root = false;
    let mut image = false;
    loop {
        match reader.read_event()? {
            Event::Start(tag) => {
                if depth == 0 {
                    ensure!(
                        !root && tag.name().as_ref() == b"DOC",
                        "Invalid KRA document root"
                    );
                    root = true;
                }
                if depth == 1 && tag.name().as_ref() == b"IMAGE" {
                    image = true;
                }
                depth += 1;
                ensure!(depth <= 256, "KRA metadata nesting limit exceeded");
            }
            Event::End(_) => depth = depth.checked_sub(1).context("Invalid KRA document")?,
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(root && image && depth == 0, "Missing Krita image");
    Ok(())
}

fn verify_snapshot(bytes: &[u8]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("Invalid KRA archive")?;
    ensure!(archive.len() <= 32768, "Too many KRA members");
    let mut mime = false;
    let mut document = false;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name()?.into_owned();
        let size = file.size();
        total = total.checked_add(size).context("KRA size overflow")?;
        ensure!(
            total <= 512 * 1024 * 1024,
            "KRA expands beyond the document limit"
        );
        let mut reader = file.take(size + 1);
        if name == "mimetype" || name == "maindoc.xml" {
            ensure!(size <= 8 * 1024 * 1024, "Invalid KRA header");
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            ensure!(data.len() as u64 == size, "Truncated KRA header");
            if name == "mimetype" {
                ensure!(
                    !mime && data == b"application/x-krita",
                    "Invalid KRA MIME type"
                );
                mime = true;
            } else {
                ensure!(!document, "Duplicate KRA document header");
                verify_document_xml(&data)?;
                document = true;
            }
        } else {
            ensure!(
                std::io::copy(&mut reader, &mut std::io::sink())? == size,
                "Truncated KRA member"
            );
        }
    }
    ensure!(mime && document, "Missing KRA document headers");
    Ok(())
}
