pub fn wave() -> [u8; 256] {
    let mut buf = [0u8; 256];
    for i in 0..128 {
        buf[i] = 0xFF;
    }
    // buf[128..256] already 0x00
    buf
}
