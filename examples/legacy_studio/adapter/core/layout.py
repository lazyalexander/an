MODEL_GROUP = "Model (protected)"
_POSE_PREFIX = "User — "


def pose_group_name(pose_id: str) -> str:
    return f"{_POSE_PREFIX}{pose_id}"


def parse_pose_group(name: str) -> str | None:
    if name.startswith(_POSE_PREFIX):
        return name[len(_POSE_PREFIX):]
    return None


def layer_name(part: str) -> str:
    return part
