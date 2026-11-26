from pathlib import Path


def transform(data):
    notes = [
        f"- Line {n}: {text}"
        for n, text in enumerate(data.decode().splitlines(), 1)
        if "TODO" in text
    ]
    return ("# Review\n\n" + "\n".join(notes or ["No TODO items."]) + "\n").encode()


if __name__ == "__main__":
    (source,) = Path("/input/document").iterdir()
    Path("/output/review.md").write_bytes(transform(source.read_bytes()))
