Name:           atomic-rollback
Version:        0.3.2
Release:        1%{?dist}
Summary:        Atomic system rollback for Fedora via Btrfs RENAME_EXCHANGE

License:        MIT OR Apache-2.0
URL:            https://github.com/rocketman-code/atomic-rollback
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

BuildRequires:  cargo
BuildRequires:  rust >= 1.85
BuildRequires:  gcc

# Runtime: the tools our code delegates to
Requires:       btrfs-progs
Requires:       grub2-tools
Requires:       dracut
Requires:       util-linux
Requires:       systemd
Requires:       libdnf5-plugin-actions

ExclusiveArch:  x86_64 aarch64

%description
Atomic system rollback for Fedora via Btrfs RENAME_EXCHANGE subvolume swap.
Migrates Fedora's default Btrfs layout to support rollback by construction,
then provides atomic rollback with a formally verified state machine.

%prep
%autosetup -n %{name}-%{version}
tar xf %{SOURCE1}
mkdir -p .cargo
cp cargo-config.toml .cargo/config.toml

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm755 hooks/90-%{name}.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-%{name}.install
install -Dm644 plugins/%{name}.actions %{buildroot}%{_sysconfdir}/dnf/libdnf5-plugins/actions.d/%{name}.actions

%files
%license LICENSE-MIT LICENSE-APACHE
%doc README.md
%{_bindir}/%{name}
%{_prefix}/lib/kernel/install.d/90-%{name}.install
%config(noreplace) %{_sysconfdir}/dnf/libdnf5-plugins/actions.d/%{name}.actions

%changelog
