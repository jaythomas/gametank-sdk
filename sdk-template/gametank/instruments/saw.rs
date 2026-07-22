pub fn wave() -> [u8; 256] {
    let mut buf = [0u8; 256];
    for i in 0..256usize {
        buf[i] = i as u8;
    }
    buf
}
