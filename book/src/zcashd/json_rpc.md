# JSON-RPC altered semantics

Zallet implements a subset of the `zcashd` JSON-RPC wallet methods. While we
have endeavoured to preserve semantics where possible, for some methods it was
necessary to make changes in order for the methods to be usable with Zallet's
wallet architecture. This page documents the semantic differences between the
`zcashd` and Zallet wallet methods.

## Changed RPC methods

### `z_listaccounts`

Changes to response:
- New `account_uuid` field.

### `z_getnewaccount`

Changes to parameters:
- New `account_name` required parameter.
- New `seedfp` optional parameter.
  - This is required if the wallet has more than one seed.

### `z_getaddressforaccount`

Changes to parameters:
- `account` parameter can be a UUID.

Changes to response:
- New `account_uuid` field.
- `account` field in response is not present if the `account` parameter is a UUID.
- The returned address is now time-based if no transparent receiver is present
  and no explicit index is requested.
- Returns an error if an empty list of receiver types is provided along with a
  previously-generated diversifier index, and the previously-generated address
  did not use the default set of receiver types.

### `listaddresses`

Changes to response:
- `imported_watchonly` includes addresses derived from imported Unified Viewing
  Keys.
- Transparent addresses for which we have BIP 44 derivation information are now
  listed in a new `derived_transparent` field (an array of objects) instead of
  the `transparent` field.

### `z_viewtransaction`

Changes to response:
- Information about all transparent inputs and outputs (which are always visible
  to the wallet) are now included. This causes the following semantic changes:
  - `pool` field on both inputs and outputs can be `"transparent"`.
  - New fields `tIn` and `tOutPrev` on inputs.
  - New field `tOut` on outputs.
  - `address` field on outputs is no longer only omitted if the output was
    received on an account-internal address; use `walletInternal` for this.
  - `memo` field on outputs is omitted if `pool = "transparent"`.
  - `memoStr` field on outputs is no longer only omitted if `memo` does not
    contain valid UTF-8.

### `z_sendmany`

Changes to parameters:
- `fee` must be `null` if set; ZIP 317 fees are always used.
- If the `minconf` field is omitted, the default ZIP 315 confirmation policy
  (3 confirmations for trusted notes, 10 confirmations for untrusted notes)
  is used.

Changes to response:
- New `txids` array field in response.
- `txid` field is omitted if `txids` has length greater than 1.

## Omitted RPC methods

The following RPC methods from `zcashd` have intentionally not been implemented
in Zallet, either due to being long-deprecated in `zcashd`, or because other RPC
methods have been updated to replace them.

| Omitted RPC method     | Use this instead |
|------------------------|------------------|
| `createrawtransaction` | [To-be-implemented methods for working with PCZTs][pczts] |
| `fundrawtransaction`   | [To-be-implemented methods for working with PCZTs][pczts] |
| `getnewaddress`        | `z_getnewaccount`, `z_getaddressforaccount` |
| `getrawchangeaddress`  |
| `keypoolrefill`        |
| `importpubkey`         |
| `importwallet`         |
| `settxfee`             |
| `signrawtransaction`   | [To-be-implemented methods for working with PCZTs][pczts] |
| `z_importwallet`       |
| `z_getbalance`         | `z_getbalanceforaccount`, `z_getbalanceforviewingkey`, `getbalance` |
| `z_getmigrationstatus` |
| `z_getnewaddress`      | `z_getnewaccount`, `z_getaddressforaccount` |
| `z_listaddresses`      | `listaddresses` |

[pczts]: https://github.com/zcash/wallet/issues/99
