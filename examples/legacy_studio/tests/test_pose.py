import pytest

from blender_addon.pose import matrix_to_pose


def test_identity_matrix():
    pose = matrix_to_pose([[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]])
    assert pose["matrix"] == [1.0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
    assert pose["location"] == [0.0, 0.0, 0.0]


def test_translation_extracted():
    pose = matrix_to_pose([[1, 0, 0, 3.5], [0, 1, 0, -2], [0, 0, 1, 7], [0, 0, 0, 1]])
    assert pose["location"] == [3.5, -2.0, 7.0]


def test_rejects_bad_shape():
    with pytest.raises(ValueError):
        matrix_to_pose([[1, 0, 0], [0, 1, 0], [0, 0, 1]])
