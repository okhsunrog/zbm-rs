# PCR allocation takes effect only after a hardware reset. Reset the disposable
# emulator before qemu-vm.nix requests TPM2_Shutdown(STATE); a pending allocation
# makes that state-save fail with TPM_RC_TYPE. Never touches a host TPM.
{ pkgs, banks }:
let
  allocation = pkgs.lib.concatMapStringsSep "+"
    (bank: "${bank}:${if builtins.elem bank banks then "all" else "none"}")
    [ "sha1" "sha256" "sha384" "sha512" ];
in ''
  tpm2_pcrallocate ${allocation}
  tpm2_shutdown --clear
  ${pkgs.swtpm}/bin/swtpm_ioctl --unix "$NIX_SWTPM_DIR/socket.ctrl" --stop
  ${pkgs.swtpm}/bin/swtpm_ioctl --unix "$NIX_SWTPM_DIR/socket.ctrl" -i
  tpm2_startup --clear
''
