// M3.2 [LLM-generated]: diagnostics of the `mem_contract` fallback attribute.
#![no_std]
#![crate_type = "lib"]
#![feature(contracts)]
#![allow(incomplete_features)]
use core::contracts::mem_contract;

unsafe extern "C" {
    #[mem_contract(requires(valid_slice::<u8>(p)))]
    //~^ ERROR invalid `mem_contract`: `valid_slice` takes 2 argument(s)
    fn a(p: *const u8, n: usize);
    #[mem_contract(requires(valid_ref::<u8>(q)))]
    //~^ ERROR invalid `mem_contract`: `valid_ref`: `q` is not a parameter of this function
    fn b(p: *const u8);
    #[mem_contract(ret = valid_ref::<u8>(p))]
    //~^ ERROR invalid `mem_contract`: `ret = valid_ref(...)`: the first argument must be `ret`
    fn c(p: *const u8) -> *const u8;
    #[mem_contract(keep(z))]
    //~^ ERROR invalid `mem_contract`: `z` is not a parameter of this function
    fn d(p: *const u8);
    #[mem_contract(frobnicate(p))]
    //~^ ERROR invalid `mem_contract`
    fn e(p: *const u8);
}

#[mem_contract(requires(valid_ref::<u8>(p)))]
//~^ ERROR `mem_contract` can only be used on functions
struct S;


