// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TrackDetail } from "../api";
import { t } from "../i18n";
import { answer, renderApp, sent, user } from "../test/render";
import { CoverCard, CoverImage, coverSrc } from "./Cover";

describe("coverSrc", () => {
  it("asks for the thumbnail or the large copy", () => {
    expect(coverSrc("/api/v1/public/covers/ab", true)).toBe("/api/v1/public/covers/ab?size=96");
    expect(coverSrc("/api/v1/public/covers/ab", false)).toBe("/api/v1/public/covers/ab");
    expect(coverSrc("/x?v=1", true)).toBe("/x?v=1&size=96");
    expect(coverSrc(null, true)).toBeNull();
  });
});

describe("CoverImage", () => {
  it("shows the placeholder without an image, or when it fails", () => {
    const { container, rerender } = renderApp(<CoverImage src={null} px={32} />);
    expect(container.querySelector(".cover-art.placeholder")).toBeTruthy();
    rerender(<CoverImage src="/cover.jpg" px={32} />);
    const image = container.querySelector("img");
    expect(image?.getAttribute("src")).toBe("/cover.jpg");
    fireEvent.error(image as HTMLImageElement);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".cover-art.placeholder")).toBeTruthy();
  });
});

function track(cover: TrackDetail["cover"], candidates: TrackDetail["cover_candidates"] = []): TrackDetail {
  return {
    id: 7,
    album: "Pastel Blues",
    title: "Sinnerman",
    file: { musicbrainz_release_id: null },
    cover,
    cover_candidates: candidates,
  } as unknown as TrackDetail;
}

describe("CoverCard", () => {
  it("says where the cover comes from and whose it is", () => {
    renderApp(
      <CoverCard
        track={track({ source: "file", detail: "folder.jpg", scope: "album", width: 1200, height: 1200, url: "/c" })}
      />,
    );
    expect(screen.getByText("folder.jpg")).toBeTruthy();
    expect(screen.getAllByText(t("cover.forAlbum")).length).toBeGreaterThan(0);
    expect(screen.getByText("1200 × 1200")).toBeTruthy();
  });

  it("uploads an image for the track only", async () => {
    const fetched = vi.fn(async () => new Response(JSON.stringify({}), { status: 200 }));
    vi.stubGlobal("fetch", fetched);
    renderApp(<CoverCard track={track(null)} />);
    expect(screen.getByText(t("cover.none"), { selector: "dd" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cover.forTrackOnly") }));
    const input = document.querySelector("input[type=file]") as HTMLInputElement;
    const image = new File([new Uint8Array([1, 2, 3])], "cover.png", { type: "image/png" });
    fireEvent.change(input, { target: { files: [image] } });
    await waitFor(() => expect(fetched).toHaveBeenCalled());
    const [url, init] = fetched.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/tracks/7/cover?scope=track");
    expect(init.method).toBe("PUT");
    expect((init.headers as Record<string, string>)["Content-Type"]).toBe("image/png");
    vi.unstubAllGlobals();
  });

  it("picks a candidate of the Cover Art Archive", async () => {
    answer({ "POST /api/v1/covers/candidates/{id}/accept": { id: 3, status: "pending" } });
    renderApp(
      <CoverCard
        track={track(null, [
          { id: 4, release_id: "r", title: "Pastel Blues", artist: "Nina Simone", score: 72, thumbnail_url: "/t" },
        ])}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: t("cover.useThis") }));
    await waitFor(() => expect(sent("POST /api/v1/covers/candidates/{id}/accept")).toHaveLength(1));
    expect(sent("POST /api/v1/covers/candidates/{id}/accept")[0]?.options).toMatchObject({
      params: { path: { id: 4 } },
      body: {},
    });
  });

  it("offers no change to viewers", () => {
    renderApp(<CoverCard track={track(null)} />, user("viewer"));
    expect(screen.queryByRole("button", { name: /Upload/ })).toBeNull();
  });
});
