/**
 * Deskfeed test addition: camera device enumeration (sf_rdpecam.c).
 *
 * SPDX-License-Identifier: Apache-2.0
 */

#ifndef FREERDP_SERVER_SAMPLE_SF_RDPECAM_H
#define FREERDP_SERVER_SAMPLE_SF_RDPECAM_H

#include "sfreerdp.h"

WINPR_ATTR_NODISCARD BOOL sf_peer_rdpecam_init(testPeerContext* context);
void sf_peer_rdpecam_uninit(testPeerContext* context);
BOOL sf_peer_rdpecam_start(testPeerContext* context);

#endif /* FREERDP_SERVER_SAMPLE_SF_RDPECAM_H */
