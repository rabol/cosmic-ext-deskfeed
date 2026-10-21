# 24. Shared folder

- **Status:** To do
- **Area:** Desktop sessions
- **Why it's manual:** Needs a real Windows machine and COSMIC's folder chooser.
- **Needs:** Your Windows machine (win-100) with Remote Desktop on, and a saved connection to it in Deskfeed. A local folder with a test file in it.
- **Background:** [ADR 0029](../adr/0029-plain-rdp-connections.md)

## Steps

1. In the connection's settings, under Shared folder, select Choose… and pick the folder.
2. Connect, and open File Explorer → This PC in Windows.
3. Create a file there.
4. Disconnect, rename the folder here, and connect again.
5. Select Stop sharing.

## Expected

- COSMIC's folder chooser opens.
- Windows shows "<folder name> on <this computer's name>" with the test file.
- The file created in Windows appears in the folder here.
- Renamed: it connects, and the row says the folder isn't there.
- Stop sharing removes it from the settings.

## Notes

The AVD half is completed but not tested: on an AVD desktop whose administrator doesn't allow drives, the row should say "Folder not shared: your organization doesn't allow drives on this desktop", and one that allows them should show the folder. Checking either needs AVD admin rights to set the host pool's drive setting.
