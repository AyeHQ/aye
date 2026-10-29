"""A directory of tests for an optional dependency that isn't installed.

pytest meets this conftest while collecting (it isn't one it loads at
start-up), so the skip skips only this directory.
"""

import pytest

pytest.importorskip("aye_suite_missing_optional_dependency")
