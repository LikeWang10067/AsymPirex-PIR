pub fn xor_in_place(dest: &mut [u8], src: &[u8]) {
    assert_eq!(dest.len(), src.len(), "XOR blocks must be of the same size");
    for (d, s) in dest.iter_mut().zip(src.iter()) {
        *d ^= *s;
    }
}

pub fn xor_blocks(left: &[u8], right: &[u8]) -> Vec<u8> {
    assert_eq!(left.len(), right.len(), "XOR blocks must be of the same size");

    left.iter()
        .zip(right.iter())
        .map(|(lhs, rhs)| lhs ^ rhs)
        .collect()
}