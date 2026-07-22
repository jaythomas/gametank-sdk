pub fn wave() -> [u8; 256] {
    let mut buf = [0u8; 256];
    for i in 0..64 {
        buf[i] = 0xFF;
    }
    // buf[64..256] already 0x00
    buf
}
