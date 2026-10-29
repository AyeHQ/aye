import os
import subprocess
import sys

from aye._find_aye import find_aye_bin


def _run() -> None:
    aye = os.fsdecode(find_aye_bin())
    if sys.platform == "win32":
        sys.exit(subprocess.run([aye, *sys.argv[1:]], check=False).returncode)
    os.execvp(aye, [aye, *sys.argv[1:]])


if __name__ == "__main__":
    _run()
