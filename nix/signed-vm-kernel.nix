# Preserve ABI/build metadata while making all NixOS boot paths, including
# Bootspec, refer to the public signed image instead of the unsigned bzImage.
{ pkgs, kernel, signedKernel }:
kernel // (pkgs.runCommand "signed-vm-kernel-${kernel.version}" {} ''
  mkdir -p "$out"
  ln -s ${signedKernel} "$out/bzImage"
'') // {
  # NixOS extends kernelPackages and calls kernel.override even without local
  # patches. Keep the signed output through that operation, but never associate
  # this signature with a different underlying kernel derivation.
  override = args:
    let next = kernel.override args;
    in assert next.drvPath == kernel.drvPath;
      import ./signed-vm-kernel.nix { inherit pkgs signedKernel; kernel = next; };
}
