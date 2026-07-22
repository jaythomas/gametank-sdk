pub fn wave() -> [u8; 256] {
    let mut buf = [0u8; 256];
    for i in 0..128usize {
        buf[i] = (i * 2) as u8;
    }
    for i in 128..256usize {
        buf[i] = (255 - (i - 128) * 2) as u8;
    }
    buf
}
