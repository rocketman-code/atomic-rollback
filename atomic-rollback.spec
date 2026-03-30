Name:           atomic-rollback
Version:        0.2.0
Release:        1%{?dist}
Summary:        Atomic system rollback for Fedora via Btrfs RENAME_EXCHANGE

License:        MIT OR Apache-2.0
URL:            https://github.com/rocketman-code/atomic-rollback
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust-packaging
BuildRequires:  cargo
BuildRequires:  rust >= 1.85

# Runtime: the tools our code delegates to (trusted axioms)
Requires:       btrfs-progs
Requires:       grub2-tools
Requires:       grubby
Requires:       dracut
Requires:       util-linux
Requires:       systemd
Requires:       libdnf5-plugin-actions

# Pure Rust + standard tools. Nothing architecture-specific.
ExclusiveArch:  x86_64 aarch64

%description
Atomic system rollback for Fedora via Btrfs RENAME_EXCHANGE subvolume swap.
Migrates Fedora's default Btrfs layout to support rollback by construction,
then provides atomic rollback with a formally verified state machine.

Preconditions:
- Fedora 43+ with Btrfs root filesystem
- UEFI boot (not legacy BIOS)
- Not already using an atomic desktop (Silverblue/Kinoite)

%prep
%autosetup

%build
%cargo_build

%check
%cargo_test

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm755 hooks/90-%{name}.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-%{name}.install
install -Dm644 plugins/%{name}.actions %{buildroot}%{_sysconfdir}/dnf/libdnf5-plugins/actions.d/%{name}.actions

%files
%license LICENSE-MIT LICENSE-APACHE
%{_bindir}/%{name}
%{_prefix}/lib/kernel/install.d/90-%{name}.install
%{_sysconfdir}/dnf/libdnf5-plugins/actions.d/%{name}.actions

%changelog
