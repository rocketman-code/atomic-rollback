# COPR SRPM creation target.
# Contract (from COPR docs): COPR invokes:
#   make -f .copr/Makefile srpm outdir="<outdir>" spec="<spec_path>"
# COPR clones the repo first. CURDIR is the clone root.
# The SRPM must be placed in $(outdir).
# git is not available inside the mock chroot.

SPECFILE := atomic-rollback.spec
NAME := atomic-rollback
VERSION := $(shell grep '^Version:' $(SPECFILE) | awk '{print $$2}')

srpm: source vendor
	rpmbuild -bs \
		--define "_sourcedir $(CURDIR)" \
		--define "_srcrpmdir $(outdir)" \
		$(SPECFILE)

source:
	tar czf /tmp/$(NAME)-$(VERSION).tar.gz \
		--transform='s,^\.,$(NAME)-$(VERSION),' \
		--exclude='.git' \
		--exclude='$(NAME)-*.tar.gz' \
		--exclude='target' \
		--exclude='vendor' \
		.
	mv /tmp/$(NAME)-$(VERSION).tar.gz $(CURDIR)/

vendor:
	dnf install -y cargo rust gcc 2>/dev/null || true
	cargo vendor > cargo-config.toml
	tar czf $(NAME)-$(VERSION)-vendor.tar.gz vendor cargo-config.toml
	rm -rf vendor cargo-config.toml
