//! ABI v1 example; compile for wasm32-unknown-unknown (no WASI or host imports).
#[unsafe(no_mangle)]
pub extern "C" fn notes_alloc(length: u32) -> u32 {
    if length > 1024 * 1024 { return 0; }
    Box::into_raw(vec![0u8; length as usize].into_boxed_slice()) as *mut u8 as u32
}
/// # Safety
/// The host must supply the pointer returned by notes_alloc and its original length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notes_run(pointer: u32, length: u32) -> u64 {
    let raw = std::ptr::slice_from_raw_parts_mut(pointer as *mut u8, length as usize);
    // SAFETY: ABI v1 host allocates and writes this exact input slice before calling.
    let input = unsafe { Box::from_raw(raw) };
    let request: serde_json::Value = serde_json::from_slice(&input).unwrap_or_default();
    let selection = request["selection"].as_str().unwrap_or_default();
    let output = match request["command"].as_str() {
        Some("uppercase") => serde_json::json!({"api_version":1,"text":selection.to_uppercase()}),
        _ => serde_json::json!({"api_version":1,"message":format!("{} source words · {} characters", selection.split_whitespace().count(),selection.chars().count())}),
    };
    let bytes = serde_json::to_vec(&output).unwrap().into_boxed_slice();
    let length = bytes.len() as u64;
    let pointer = Box::into_raw(bytes) as *mut u8 as u64;
    (pointer << 32) | length
}
