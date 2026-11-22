import enum


class Phase(enum.Enum):
    ADJUSTMENT = enum.auto()
    LOCKED = enum.auto()


class InvalidTransition(RuntimeError):
    pass


class NoPose(RuntimeError):
    pass
