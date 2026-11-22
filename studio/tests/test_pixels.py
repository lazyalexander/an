import pytest

from adapter.core.pixels import rgba8888_to_krita_u8


def test_swaps_red_and_blue_per_pixel():
    # Krita RGBA/U8 pixel memory is B,G,R,A (api.kde.org: "the actual order
    # of channels is most often BGR"); QImage.Format_RGBA8888 is R,G,B,A.
    rgba = bytes([255, 0, 0, 255, 0, 255, 0, 128])
    assert rgba8888_to_krita_u8(rgba) == bytes([0, 0, 255, 255, 0, 255, 0, 128])


def test_empty_input_passes_through():
    assert rgba8888_to_krita_u8(b"") == b""


def test_rejects_misaligned_data():
    with pytest.raises(ValueError):
        rgba8888_to_krita_u8(b"\x00\x01")
