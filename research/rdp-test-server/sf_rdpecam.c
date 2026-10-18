/**
 * Deskfeed test addition: the camera device enumeration channel
 * (MS-RDPECAM 2.2.2), so camera redirection can be checked without Azure.
 *
 * The server opens "RDCamera_Device_Enumerator". A client with camera
 * redirection answers with a Select Version Request; the server replies and
 * the client then lists its cameras (Device Added Notification). Each step
 * is logged with DESKFEED-TEST. A client without camera redirection refuses
 * the channel, so neither line appears.
 *
 * Only enumeration: no device channels or video samples.
 *
 * SPDX-License-Identifier: Apache-2.0
 */

#include <freerdp/config.h>

#include <winpr/assert.h>
#include <winpr/string.h>

#include "sfreerdp.h"

#include "sf_rdpecam.h"

#include <freerdp/log.h>
#define TAG SERVER_TAG("sample.rdpecam")

WINPR_ATTR_NODISCARD
static UINT sf_rdpecam_select_version_request(CamDevEnumServerContext* context,
                                              const CAM_SELECT_VERSION_REQUEST* request)
{
	WINPR_ASSERT(context);
	WINPR_ASSERT(request);

	WLog_INFO(TAG, "DESKFEED-TEST camera channel: client protocol version %" PRIu8,
	          request->Header.Version);

	/* Version 2 is the newest (MS-RDPECAM 2.2.1); answer with the client's
	 * if it is older. */
	CAM_SELECT_VERSION_RESPONSE response = WINPR_C_ARRAY_INIT;
	response.Header.Version = request->Header.Version < 2 ? request->Header.Version : 2;
	response.Header.MessageId = CAM_MSG_ID_SelectVersionResponse;
	return context->SelectVersionResponse(context, &response);
}

WINPR_ATTR_NODISCARD
static UINT sf_rdpecam_device_added(WINPR_ATTR_UNUSED CamDevEnumServerContext* context,
                                    const CAM_DEVICE_ADDED_NOTIFICATION* notification)
{
	WINPR_ASSERT(notification);

	char* name = ConvertWCharToUtf8Alloc(notification->DeviceName, nullptr);
	WLog_INFO(TAG, "DESKFEED-TEST camera offered: \"%s\" (channel %s)", name ? name : "?",
	          notification->VirtualChannelName ? notification->VirtualChannelName : "?");
	free(name);
	return CHANNEL_RC_OK;
}

WINPR_ATTR_NODISCARD
static UINT sf_rdpecam_device_removed(WINPR_ATTR_UNUSED CamDevEnumServerContext* context,
                                      const CAM_DEVICE_REMOVED_NOTIFICATION* notification)
{
	WINPR_ASSERT(notification);

	WLog_INFO(TAG, "DESKFEED-TEST camera removed: channel %s",
	          notification->VirtualChannelName ? notification->VirtualChannelName : "?");
	return CHANNEL_RC_OK;
}

BOOL sf_peer_rdpecam_init(testPeerContext* context)
{
	WINPR_ASSERT(context);

	context->rdpecam = cam_dev_enum_server_context_new(context->vcm);
	if (!context->rdpecam)
		return FALSE;

	context->rdpecam->rdpcontext = &context->_p;
	context->rdpecam->userdata = context;
	context->rdpecam->SelectVersionRequest = sf_rdpecam_select_version_request;
	context->rdpecam->DeviceAddedNotification = sf_rdpecam_device_added;
	context->rdpecam->DeviceRemovedNotification = sf_rdpecam_device_removed;
	return TRUE;
}

BOOL sf_peer_rdpecam_start(testPeerContext* context)
{
	if (!context || !context->rdpecam || !context->rdpecam->Open)
		return FALSE;

	const UINT rc = context->rdpecam->Open(context->rdpecam);
	if (rc != CHANNEL_RC_OK)
		WLog_INFO(TAG, "DESKFEED-TEST camera channel open failed: %" PRIu32, rc);
	return rc == CHANNEL_RC_OK;
}

void sf_peer_rdpecam_uninit(testPeerContext* context)
{
	if (!context || !context->rdpecam)
		return;

	if (context->rdpecam_open && context->rdpecam->Close)
		(void)context->rdpecam->Close(context->rdpecam);
	cam_dev_enum_server_context_free(context->rdpecam);
	context->rdpecam = nullptr;
}
