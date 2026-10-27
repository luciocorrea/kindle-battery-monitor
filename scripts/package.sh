#!/bin/sh
set -eu
project=$(CDPATH= cd -P "$(dirname "$0")/.." && pwd)
python=${PYTHON:-python3}
"$python" -c 'import sys; sys.exit(sys.version_info < (3, 11))' || {
    echo 'Packaging requires Python 3.11 or newer; set PYTHON to its executable.' >&2
    exit 1
}
exec "$python" "$project/packaging/release.py" "$@"
