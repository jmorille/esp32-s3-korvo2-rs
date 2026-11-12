// SPDX-License-Identifier: MIT OR Apache-2.0
fn main() {
    // Propagate the ESP-IDF link arguments / cfgs to the examples.
    embuild::espidf::sysenv::output();
}
