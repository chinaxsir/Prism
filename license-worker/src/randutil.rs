//! CSPRNG：走 Workers 全局 crypto.getRandomValues（激活码、jti 用）。

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};

fn crypto() -> web_sys::Crypto {
    Reflect::get(&js_sys::global(), &JsValue::from_str("crypto"))
        .expect("globalThis.crypto unavailable")
        .unchecked_into::<web_sys::Crypto>()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let arr = Uint8Array::new_with_length(n as u32);
    crypto()
        .get_random_values_with_array_buffer_view(&arr)
        .expect("crypto.getRandomValues failed");
    arr.copy_to(&mut buf);
    buf
}

pub fn random_u64() -> u64 {
    let b = random_bytes(8);
    u64::from_be_bytes(b.try_into().unwrap())
}
