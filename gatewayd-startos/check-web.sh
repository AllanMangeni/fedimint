#!/bin/bash

DURATION=$(</dev/stdin)
if ((DURATION <= 5000)); then
    exit 60
else
    if ! curl --silent --fail -L http://fedimint-gatewayd.embassy:8176 &>/dev/null; then
        echo "Web interface is unreachable" >&2
        exit 1
    fi
fi
