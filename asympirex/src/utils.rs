pub fn xor_in_place(dest: &mut [u8], src: &[u8]) {
    assert_eq!(dest.len(), src.len(), "XOR blocks must be of the same size");
    for (d, s) in dest.iter_mut().zip(src.iter()) {
        *d ^= *s;
    }
}