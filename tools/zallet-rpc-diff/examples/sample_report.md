# Zallet Parity Report

- **Total Tests**: 7
- **✅ Matches**: 3
- **❌ Diffs**: 1
- **📋 Expected Diffs**: 2
- **🔍 Missing**: 1
- **⚠️ Errors**: 0

## Detailed Results

| Method | Status | Details |
| :--- | :--- | :--- |
| `getbalance` | ✅ Match | |
| `getblockchaininfo` | ✅ Match | |
| `getdeprecationinfo` | 📋 Expected Diff | 3 field(s): `/version`, `/deprecationheight`, `/end_of_service` — _zcashd's getdeprecationinfo is tied to its release/deprecation cycle; Zallet returns different values or may not implement this method yet._ |
| `getnetworkinfo` | 📋 Expected Diff | 2 field(s): `/subversion`, `/version` — _Zallet reports its own agent/version string (e.g. 'Zallet/0.1.0') while zcashd reports '/MagicBean:...' — intentional product identity difference._ |
| `getwalletinfo` | ✅ Match | |
| `z_gettotalbalance` | ❌ Diff | 1 field(s) differ: `/private` |
| `z_listaddresses` | 🔍 Missing | Method `z_listaddresses` not found on one endpoint |
