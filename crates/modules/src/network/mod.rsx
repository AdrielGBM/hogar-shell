[logic]
//! The network panel: the radio, the networks in range, and joining one.
//!
//! The bar chip stays what it was — a sysfs link verdict that needs no NetworkManager — and this panel is the NetworkManager view layered on top. So a machine without NM keeps a working chip and gets a panel that says why it is empty, rather than the chip going blank because the panel's dependency is missing.
