# 0024. Connection-file properties: a rule per property, unused ones dropped

- Status: Accepted (review F04)
- Date: 2026-10-19

## Context

`rdpfile.rs` kept a list of property names from the feed's connection file. Any printable string or any integer was passed on. Only `gatewayhostname` was checked as a destination: it must be an AVD service host. The external review found that `full address`, `alternate full address` and `kdcproxyname` were passed on unchecked, and that types and ranges weren't enforced. Spec §7 asks for typed, reviewed properties and separate destination checks.

FreeRDP 3.32.2, checked in its source:
- `kdcproxyname` becomes the Kerberos KDC address as given (`client/common/file.c`, `FreeRDP_KerberosKdcUrl`).
- With the AVD gateway (`/gateway:type:arm`, always set), the session host FreeRDP connects to comes from the broker's answer (`libfreerdp/core/gateway/arm.c`, `FreeRDP_ServerHostname`), not from `full address`.
- `armpath`, `geo`, `hubdiscoverygeourl` and `wvd endpoint pool` are read from the file but used nowhere. `resourceprovider:s:arm` does have an effect: it sets `GatewayArmTransport` (`file.c`, used by `wst.c`). The launcher's `/gateway:type:arm` sets the same thing (`cmdline.c`), so dropping it changes nothing. *(Corrected 2026-10-20: this line first said `resourceprovider` was unused.)* `workspace id` and `remotedesktopname` aren't read at all.

## Decision

- Each kept property has a rule (`rdpfile::rule`):
  - **Integers** must be `i` and within the range in Microsoft's RDP property reference. Mostly `0`–`1`; `audiomode` `0`–`2`; `gatewayusagemethod` `0`–`4`; `gatewaycredentialssource` `0`–`5`. `gatewaybrokeringtype` is undocumented, so `0`–`9`.
  - **`full address`, `alternate full address` and `gatewayhostname`:** a host name or IPv4 address with an optional port, or `[IPv6]:port`. No scheme, user name or path. The gateway check against AVD's hosts stays.
  - **`kdcproxyname`:** a plain host name.
  - **Text values** (`loadbalanceinfo`, `aadtenantid`, the RemoteApp program, name and arguments): printable and bounded.
- The unused properties above are no longer passed on.
- A kept property whose value breaks its rule is dropped, and its *name* goes to the log (`connection file: dropped for their form: …`). Values never do. This way a real feed with an unexpected form shows up in the log, instead of a connection failing without a trace.

## Consequences

- Host names may contain `-` and `_` and any label, so GUID-like names pass. A form not foreseen here shows in the log. A missing or refused `full address` ends with the existing "connection details couldn't be read" message.
- Tests check every rule both ways (`each_property_has_its_form`). The random-input test checks that only ruled properties reach FreeRDP. The real-FreeRDP test still accepts every kept property.
