# Zallet Docker Image

This Docker image provides a containerized version of Zallet, a wallet for Zcash.

## Quick Start

```bash
# Create a data directory for persistent storage
mkdir -p ./zallet-data

# Create the identity file (required for wallet encryption)
age-keygen -o ./zallet-data/identity.txt

# Run the container
docker run -v $(pwd)/zallet-data:/home/zallet/.data zcash/zallet
```

## Understanding the Identity File

### What is the identity file?

The identity file is a cryptographic key file used by Zallet to encrypt and decrypt sensitive wallet information such as:

- Seed phrases
- Private keys
- Other key material

It uses the [age encryption](https://age-encryption.org/) framework, which is a modern file encryption tool.

### Why is it important?

- **Without this file, you cannot access your funds** - the identity file is required to decrypt your wallet data
- If you lose this file, you won't be able to recover your wallet unless you have a backup of the seed phrase
- This file must be securely stored and backed up

## Creating an Identity File

Before running the Zallet container, you must create an identity file on your host system.

### Option 1: Unencrypted Identity File (Basic)

```bash
# Install age encryption tools
# On Ubuntu/Debian: apt install age
# On macOS: brew install age

# Generate an identity file
age-keygen -o /path/on/host/identity.txt
```

### Option 2: Passphrase-protected Identity File (Recommended for production)

```bash
# Generate a passphrase-protected identity file
age -p -o /path/on/host/identity.txt <(age-keygen)
```

With this approach, you'll need to enter a passphrase when Zallet needs to decrypt wallet material.

## Mounting the Identity File

When running the Docker container, you must mount the identity file into the container:

```bash
docker run -v /path/on/host/identity.txt:/home/zallet/.data/identity.txt zcash/zallet
```

Alternatively, mount a data directory containing the identity file:

```bash
docker run -v /path/on/host/data-dir:/home/zallet/.data zcash/zallet
```

## Security Best Practices

1. **Create backups** of your identity file and store them securely
2. **Use passphrase protection** for production environments
3. **Restrict file permissions** on your host system: `chmod 600 identity.txt`
4. **Never share** your identity file
5. Consider using hardware-backed age plugins for enhanced security in production environments

## Docker Compose Example

Here's a complete docker-compose.yml example:

```yaml
version: '3'

services:
  zallet:
    image: zcash/zallet
    volumes:
      - ./data:/home/zallet/.data
    environment:
      - RUST_LOG=info
      - ZALLET_NETWORK=main
      - ZALLET_RPC__BIND=0.0.0.0:28232
    ports:
      - "28232:28232"
```

Make sure to create the identity file in the `./data` directory before starting:

```bash
mkdir -p ./data
age-keygen -o ./data/identity.txt
```

Then start the container:

```bash
docker-compose up -d
```

## Initializing the Wallet

After starting the container with your identity file, you need to:

1. Initialize wallet encryption:

```bash
docker exec -it zallet_container zallet init-wallet-encryption
```

2. Generate a mnemonic phrase:

```bash
docker exec -it zallet_container zallet generate-mnemonic
```

For more information on Zallet commands, see the [Zallet documentation](../README.md).
