# Debian binary packages setup

The Electric Coin Company operates a package repository for 64-bit Debian-based
distributions. If you'd like to try out the binary packages, you can set it up on your
system and install Zallet from there.

First install the following dependency so you can talk to our repository using HTTPS:

```bash
sudo apt-get update && sudo apt-get install apt-transport-https wget gnupg2
```

Next add the Zcash master signing key to apt's trusted keyring:

```bash
wget -qO - https://apt.z.cash/zcash.asc | gpg --import
gpg --export B1C9095EAA1848DBB54D9DDA1D05FDC66B372CFE | sudo apt-key add -
```

```
Key fingerprint = B1C9 095E AA18 48DB B54D 9DDA 1D05 FDC6 6B37 2CFE
```

Add the repository to your Bullseye sources:

```bash
echo "deb [arch=amd64] https://apt.z.cash/ bullseye main" | sudo tee /etc/apt/sources.list.d/zcash.list
```

Or add the repository to your Bookworm sources:

```bash
echo "deb [arch=amd64] https://apt.z.cash/ bookworm main" | sudo tee /etc/apt/sources.list.d/zcash.list
```

Update the cache of sources and install Zcash:

```bash
sudo apt update && sudo apt install zallet
```

## Troubleshooting

### Missing Public Key Error

If you see:

```
The following signatures couldn't be verified because the public key is not available: NO_PUBKEY B1C9095EAA1848DB
```

Get the new key directly from the [z.cash site](https://apt.z.cash/zcash.asc):

```bash
wget -qO - https://apt.z.cash/zcash.asc | gpg --import
gpg --export B1C9095EAA1848DBB54D9DDA1D05FDC66B372CFE | sudo apt-key add -
```

to retrieve the new key and resolve this error.

### Revoked Key error

If you see something similar to:

```
The following signatures were invalid: REVKEYSIG AEFD26F966E279CD
```

Remove the key marked as revoked:

```bash
sudo apt-key del AEFD26F966E279CD
```

Then retrieve the updated key:

```bash
wget -qO - https://apt.z.cash/zcash.asc | gpg --import
gpg --export B1C9095EAA1848DBB54D9DDA1D05FDC66B372CFE | sudo apt-key add -
```

Then update the list again:

```bash
sudo apt update
```

### Expired Key error

If you see something similar to:

```
The following signatures were invalid: KEYEXPIRED 1539886450
```

Remove the old signing key:

```bash
sudo apt-key del 1539886450
```

Remove the list item from local apt:

```bash
sudo rm /etc/apt/sources.list.d/zcash.list
```

Update the repository list:

```bash
sudo apt update
```

Then start again at the beginning of this document.
