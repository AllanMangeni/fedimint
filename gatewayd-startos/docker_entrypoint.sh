#!/bin/bash

set -e

echo "Waiting for Start9 config..."
while [ ! -f /start-os/start9/config.yaml ]; do
    sleep 1
done

echo "Config file found at /start-os/start9/config.yaml"

export FM_GATEWAY_DATA_DIR=/gatewayd
export FM_GATEWAY_NETWORK=bitcoin
export FM_GATEWAY_LISTEN_ADDR=0.0.0.0:8176
export FM_GATEWAY_IROH_LISTEN_ADDR=0.0.0.0:8177

# Parse Lightning backend configuration
LIGHTNING_BACKEND=$(yq '.gatewayd-lightning-backend.backend-type' /start-os/start9/config.yaml)

if [ "$LIGHTNING_BACKEND" = "lnd" ]; then
    echo "Using LND backend"

    # LND files are mounted at /mnt/lnd from the LND package
    LND_DIR="/mnt/lnd"

    # Set LND RPC address (LND runs on lnd.embassy)
    export FM_LND_RPC_ADDR="https://lnd.embassy:10009"

    # Set TLS cert path - LND exposes tls.cert in its public directory
    if [ -f "${LND_DIR}/tls.cert" ]; then
        export FM_LND_TLS_CERT="${LND_DIR}/tls.cert"
        echo "Using LND TLS cert: ${FM_LND_TLS_CERT}"
    else
        echo "ERROR: LND TLS certificate not found at ${LND_DIR}/tls.cert"
        exit 1
    fi

    # Set admin macaroon path - LND exposes admin.macaroon in its public directory
    if [ -f "${LND_DIR}/admin.macaroon" ]; then
        export FM_LND_MACAROON="${LND_DIR}/admin.macaroon"
        echo "Using LND macaroon: ${FM_LND_MACAROON}"
    else
        echo "ERROR: LND admin macaroon not found at ${LND_DIR}/admin.macaroon"
        exit 1
    fi

    GATEWAY_MODE="lnd"
    echo "LND configuration complete"

elif [ "$LIGHTNING_BACKEND" = "ldk" ]; then
    echo "Using LDK backend"

    # Parse LDK configuration
    LDK_ALIAS=$(yq '.gatewayd-lightning-backend.alias' /start-os/start9/config.yaml)
    if [ -n "$LDK_ALIAS" ] && [ "$LDK_ALIAS" != "null" ]; then
        export FM_LDK_ALIAS="$LDK_ALIAS"
    else
        export FM_LDK_ALIAS="Fedimint LDK Gateway"
    fi
    echo "LDK Node Alias: $FM_LDK_ALIAS"

    export FM_PORT_LDK=10010
    GATEWAY_MODE="ldk"
else
    echo "ERROR: Unknown Lightning backend type: $LIGHTNING_BACKEND"
    exit 1
fi

# Parse Bitcoin backend configuration
BITCOIN_BACKEND=$(yq '.gatewayd-bitcoin-backend.backend-type' /start-os/start9/config.yaml)

if [ "$BITCOIN_BACKEND" = "bitcoind" ]; then
    echo "Using Bitcoin Core backend"
    BITCOIN_USER=$(yq '.gatewayd-bitcoin-backend.user' /start-os/start9/config.yaml)
    BITCOIN_PASS=$(yq '.gatewayd-bitcoin-backend.password' /start-os/start9/config.yaml)

    if [ -z "$BITCOIN_USER" ] || [ -z "$BITCOIN_PASS" ]; then
        echo "ERROR: Could not parse Bitcoin RPC credentials from config"
        exit 1
    fi

    export FM_BITCOIND_URL="http://bitcoind.embassy:8332"
    export FM_BITCOIND_USERNAME="${BITCOIN_USER}"
    export FM_BITCOIND_PASSWORD="${BITCOIN_PASS}"

    echo "Starting Gateway with Bitcoin Core at $FM_BITCOIND_URL"
elif [ "$BITCOIN_BACKEND" = "esplora" ]; then
    echo "Using Esplora backend"
    ESPLORA_URL=$(yq '.gatewayd-bitcoin-backend.url' /start-os/start9/config.yaml)

    if [ -z "$ESPLORA_URL" ]; then
        echo "ERROR: Could not parse Esplora URL from config"
        exit 1
    fi

    export FM_ESPLORA_URL="$ESPLORA_URL"
    echo "Starting Gateway with Esplora at $ESPLORA_URL"
else
    echo "ERROR: Unknown Bitcoin backend type: $BITCOIN_BACKEND"
    exit 1
fi

# Parse and hash the password
PLAINTEXT_PASSWORD=$(yq '.gatewayd-password' /start-os/start9/config.yaml)
if [ -z "$PLAINTEXT_PASSWORD" ]; then
    echo "ERROR: Gateway password not set in config"
    exit 1
fi

echo "Hashing gateway password..."
BCRYPT_HASH=$(gateway-cli create-password-hash "$PLAINTEXT_PASSWORD")
export FM_GATEWAY_BCRYPT_PASSWORD_HASH="$BCRYPT_HASH"

# Read and set RUST_LOG from config
RUST_LOG_LEVEL=$(yq '.advanced.rust-log-level' /start-os/start9/config.yaml)
export RUST_LOG="${RUST_LOG_LEVEL}"
echo "Setting RUST_LOG=${RUST_LOG}"

# Find the entrypoint script dynamically
ENTRYPOINT_SCRIPT=$(find /nix/store -type f -name '*-gatewayd-container-entrypoint.sh' | head -n 1)

echo "Starting gateway with ${GATEWAY_MODE} backend..."

if [[ -z "$ENTRYPOINT_SCRIPT" ]]; then
    echo "Entrypoint script not found, running gatewayd directly"
    exec gatewayd "$GATEWAY_MODE"
else
    echo "Using entrypoint: $ENTRYPOINT_SCRIPT"
    exec bash "$ENTRYPOINT_SCRIPT" "$GATEWAY_MODE"
fi
