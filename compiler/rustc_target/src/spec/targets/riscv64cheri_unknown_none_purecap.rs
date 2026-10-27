use crate::spec::{
    Arch, Cc, CfgAbi, LinkerFlavor, Lld, LlvmAbi, PanicStrategy, RelocModel, Target,
    TargetMetadata, TargetOptions, cvs,
};

/// Bare-metal CHERI-RISC-V (ISAv9) RV64 pure-capability target, e.g. for CHERI-Toooba,
/// CHERI-Flute, Sail-CHERI-RISC-V or QEMU-CHERI. Equivalent to clang
/// `--target=riscv64-unknown-elf -march=rv64gcxcheri -mabi=l64pc128d`.
pub(crate) fn target() -> Target {
    Target {
        // Must match clang's data layout for `-mabi=l64pc128d`: 128-bit capabilities with a 64-bit
        // address/index width in address space 200, which is also the alloca, program and
        // globals address space.
        data_layout: "e-m:e-p:64:64-pfe200:128:128:128:64-i64:64-i128:128-n32:64-S128-A200-P200-G200"
            .into(),
        llvm_target: "riscv64-unknown-elf".into(),
        metadata: TargetMetadata {
            description: Some("Bare RISC-V (RV64GC) CHERI pure-capability (ISAv9)".into()),
            tier: Some(3),
            host_tools: Some(false),
            std: Some(false),
        },
        pointer_width: 128,
        arch: Arch::RiscV64,

        options: TargetOptions {
            address_width: Some(64),
            linker_flavor: LinkerFlavor::Gnu(Cc::No, Lld::Yes),
            linker: Some("rust-lld".into()),
            cpu: "generic-rv64".into(),
            cfg_abi: CfgAbi::Purecap,
            llvm_abiname: LlvmAbi::L64pc128d,
            max_atomic_width: Some(64),
            atomic_cas: true,
            features: "+m,+a,+f,+d,+c,+zicsr,+zifencei,+xcheri,+xcheripurecap".into(),
            panic_strategy: PanicStrategy::Abort,
            relocation_model: RelocModel::Static,
            emit_debug_gdb_scripts: false,
            eh_frame_header: false,
            families: cvs!["cheri"],
            is_like_cheri: true,
            default_address_space: rustc_abi::AddressSpace(200),
            ..Default::default()
        },
    }
}
