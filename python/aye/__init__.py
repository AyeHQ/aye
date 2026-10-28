"""aye: a fast, pytest-compatible test runner. The work happens in the `aye`
binary; this package only exists so `python -m aye` can find it."""

from aye._find_aye import find_aye_bin

__all__ = ["find_aye_bin"]
