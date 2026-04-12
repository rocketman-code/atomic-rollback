Name:           atomic-rollback
Version:        0.3.8
Release:        1%{?dist}
Summary:        Atomic system rollback for Fedora via Btrfs RENAME_EXCHANGE

License:        GPL-3.0-only AND (MIT OR Apache-2.0) AND MIT AND Unicode-3.0
URL:            https://github.com/rocketman-code/atomic-rollback
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

BuildRequires:  cargo
BuildRequires:  rust >= 1.85
BuildRequires:  gcc
BuildRequires:  rpm-devel

# Runtime: the tools our code delegates to
Requires:       btrfs-progs
Requires:       grub2-tools
Requires:       dracut
Requires:       util-linux
Requires:       systemd

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
gcc -shared -fPIC -o atomic_rollback.so plugins/atomic_rollback.c $(pkg-config --cflags --libs rpm)

%check
cargo test --release --offline

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm755 atomic_rollback.so %{buildroot}%{__plugindir}/atomic_rollback.so
install -Dm644 plugins/macros.transaction_atomic_rollback %{buildroot}/usr/lib/rpm/macros.d/macros.transaction_atomic_rollback
install -Dm644 %{name}.conf %{buildroot}%{_sysconfdir}/%{name}.conf

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{__plugindir}/atomic_rollback.so
/usr/lib/rpm/macros.d/macros.transaction_atomic_rollback
%config(noreplace) %{_sysconfdir}/%{name}.conf

%posttrans -p /bin/sh
/usr/bin/atomic-rollback ensure-hooks 2>/dev/null || :
exit 0

%changelog
