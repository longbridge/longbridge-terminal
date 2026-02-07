pub fn hexa_to_rgb(hex_code: String) -> (u8, u8, u8) {
    let hex_code = hex_code.trim_start_matches('#');
    let r = u8::from_str_radix(&hex_code[0..2], 16).unwrap();
    let g = u8::from_str_radix(&hex_code[2..4], 16).unwrap();
    let b = u8::from_str_radix(&hex_code[4..6], 16).unwrap();
    (r, g, b)
}
