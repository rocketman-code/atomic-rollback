#include <rpm/rpmlog.h>
#include <rpm/rpmts.h>
#include <rpm/rpmplugin.h>
#include <stdlib.h>
#include <unistd.h>

#define BINARY "/usr/bin/atomic-rollback"

static rpmRC atomic_rollback_tsm_pre(rpmPlugin plugin, rpmts ts)
{
    if (rpmtsFlags(ts) & (RPMTRANS_FLAG_TEST|RPMTRANS_FLAG_BUILD_PROBS))
        return RPMRC_OK;

    if (access(BINARY, X_OK) != 0)
        return RPMRC_OK;

    int rc = system(BINARY " snapshot");
    if (rc != 0) {
        rpmlog(RPMLOG_ERR, "atomic-rollback: snapshot failed, aborting transaction\n");
        return RPMRC_FAIL;
    }
    return RPMRC_OK;
}

struct rpmPluginHooks_s atomic_rollback_hooks = {
    .tsm_pre = atomic_rollback_tsm_pre,
};
