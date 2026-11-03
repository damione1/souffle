// SPDX-License-Identifier: GPL-3.0-or-later
// Pin each published download link to the release it actually downloads.
export default async function () {
  const headers = {Accept: "application/vnd.github+json", "User-Agent": "souffle-website-build"};
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const response = await fetch("https://api.github.com/repos/damione1/souffle/releases/latest", {
    headers, signal: AbortSignal.timeout(15000),
  });
  if (!response.ok) throw new Error(`Cannot resolve the published Soufflé release (${response.status})`);
  const release = await response.json();
  if (release.draft || release.prerelease || !/^v?\d+\.\d+\.\d+$/.test(release.tag_name)) {
    throw new Error("The website download must target a published stable release");
  }
  const installers = release.assets.filter(asset => /\.dmg$/i.test(asset.name) && asset.size > 0);
  if (installers.length !== 1) throw new Error("Expected exactly one published macOS installer");
  const url = new URL(installers[0].browser_download_url);
  if (url.origin !== "https://github.com" ||
      !url.pathname.startsWith(`/damione1/souffle/releases/download/${release.tag_name}/`)) {
    throw new Error("The installer URL does not belong to the selected release");
  }
  return {version: release.tag_name.replace(/^v/, ""), url: url.href,
    platform: "macos", architecture: "arm64", channel: "stable"};
}
