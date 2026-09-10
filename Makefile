# Local Nightly builds coexist with the installed app (com.souffle.desktop.nightly).

.PHONY: help nightly nightly-dmg

help:
	@echo "make nightly      build Soufflé Nightly (debug) and open it"
	@echo "make nightly-dmg  same, wrapped in a .dmg"

nightly:
	./scripts/run-nightly.sh

nightly-dmg:
	./scripts/run-nightly.sh --dmg
