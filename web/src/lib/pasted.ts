/** What a paste or a drop needs to read: a `DataTransfer`, or just its files and items. */
type Carrier = Pick<DataTransfer, "files" | "items">;

/**
 * The files a paste or a drop carries: files copied in Finder or Explorer,
 * or a picture (a screenshot, an image copied in a browser), which browsers
 * hand over as a file. Most list them in `files`; where one only puts them
 * in `items`, they're read from there. None means it's text, for the box.
 */
export function filesIn(data: Carrier | null | undefined): File[] {
  if (!data) return [];
  const files = Array.from(data.files ?? []);
  if (files.length) return files;
  return Array.from(data.items ?? [])
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => !!file);
}
