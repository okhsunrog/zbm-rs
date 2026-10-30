# Optional preflight for the opt-in backport, using the real kernel toolchain
# and configuration without building every unrelated driver first.
{ kernel }:
kernel.overrideAttrs (_: {
  name = "linux-${kernel.version}-tpm-hibernation-compile-check";
  outputs = [ "out" ];
  outputChecks = {};
  # Kernel buildFlags contains bzImage; passing it here would build the full
  # kernel instead of checking just the affected subsystems.
  buildPhase = ''
    runHook preBuild
    for option in EFI EFI_STUB HIBERNATION HIBERNATION_TPM_SIGNATURE \
      TCG_TPM TCG_TPM2_HMAC TCG_TPM_KERNEL_NVINDEX TCG_TPM2_KERNEL_KEY \
      KERNEL_TPM_SECURITY KEXEC_SIG_FORCE MODULE_SIG_FORCE \
      LOCK_DOWN_KERNEL_FORCE_INTEGRITY; do
      grep -Fx "CONFIG_$option=y" .config
    done
    grep -Fx '# CONFIG_TPM_HIBERNATE_INSECURE is not set' .config
    make "''${makeFlags[@]}" -j "$NIX_BUILD_CORES" \
      drivers/char/tpm/ drivers/firmware/efi/ kernel/power/
    runHook postBuild
  '';
  preInstall = "";
  installPhase = ''
    mkdir -p "$out"
    cp .config "$out/config"
    find drivers/char/tpm drivers/firmware/efi kernel/power -name '*.o' -type f | sort > "$out/compiled-objects"
    test -s "$out/compiled-objects"
  '';
  postInstall = "";
  preFixup = "";
  dontFixup = true;
})
