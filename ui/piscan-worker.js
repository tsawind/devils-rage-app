// PI scan: the slow automatic lock-on runs here so the page stays smooth.
importScripts("piscan-engine.js");
onmessage = (e) => {
  const { id, markers, colonies, disc } = e.data;
  let pose = null;
  try { pose = self.PiScanEngine.autoLock(markers, colonies, disc); } catch (err) { pose = null; }
  postMessage({ id, pose });
};
