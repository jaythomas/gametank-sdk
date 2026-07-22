pub fn wave() -> [u8; 256] {
    let mut buf = [0u8; 256];
    for i in 0..256usize {
        let radians = core::f32::consts::TAU * i as f32 / 256.0;
        buf[i] = (radians.sin() * 127.0 + 128.0) as u8;
    }
    buf
}
