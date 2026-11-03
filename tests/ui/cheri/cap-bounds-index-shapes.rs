// Indexing shapes that `-Z cheri-cap-bounds-checks` must compile without ICE: codegen
// asserts that every site whose check it takes over is accessed through the narrowed
// capability.
//
//@ build-pass
//@ revisions: narrow elide narrow0 elide0
//@ [narrow] compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=narrow
//@ [elide] compile-flags: -Copt-level=2 -Zcheri-cap-bounds-checks=elide
//@ [narrow0] compile-flags: -Copt-level=0 -Zcheri-cap-bounds-checks=narrow
//@ [elide0] compile-flags: -Copt-level=0 -Zcheri-cap-bounds-checks=elide
//@ only-riscv64cheri-unknown-none-purecap

#![crate_type = "lib"]
#![no_std]

pub struct S {
    pub pad: u8,
    pub arr: [u16; 8],
    pub grid: [[u32; 4]; 4],
}

#[derive(Clone, Copy)]
pub enum E {
    A(u32),
    B,
}

pub fn shapes(
    s: &mut [u32],
    t: &[u64],
    a: &mut [u8; 32],
    st: &mut S,
    es: &[E],
    big: &[u8; 5000],
    zs: &[()],
    pairs: &mut [(u8, ())],
    p: *const [u16],
    i: usize,
    j: usize,
) -> u64 {
    let mut acc = 0u64;
    s[i] = s[j];
    s[i] += 1;
    acc += t[i] + t[3];
    a[i] = a[j].wrapping_mul(3);
    st.arr[i] = st.arr[j];
    st.grid[i][j] += 2;
    acc += st.grid[j][i] as u64;
    if let E::A(x) = es[i] {
        acc += x as u64;
    }
    acc += big[i] as u64;
    zs[i];
    pairs[i].1 = ();
    pairs[j].0 = 1;
    let r = &s[i];
    acc += *r as u64;
    let mut local = [0u64; 16];
    local[i] = acc;
    acc += local[j];
    acc += unsafe { (*p)[i] } as u64;
    acc += generic(t, i) + generic(&[1u8, 2, 3][..], j) as u64;
    acc += (|| t[j])();
    acc
}

#[inline(never)]
pub fn generic<T: Copy>(s: &[T], i: usize) -> T {
    s[i]
}

pub fn indirect(s: &&[u32], i: usize) -> u32 {
    s[i]
}

pub fn nested_slices(s: &[&[u8]], i: usize, j: usize) -> u8 {
    s[i][j]
}

pub fn in_loop(s: &mut [i32]) {
    for i in 0..s.len() {
        s[i] = -s[i];
    }
}
