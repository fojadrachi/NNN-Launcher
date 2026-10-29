namespace PackLink.Core;

public record DlItem(string Url, string Path, string? Sha1 = null, long Size = 0);

public record Progress(string Stage, int Done, int Total, string? Detail = null);

static class Downloader
{
    /// Lädt alle fehlenden/kaputten Dateien parallel. Vorhandene Dateien werden per Größe
    /// (schnell) bzw. – wenn keine Größe bekannt ist – per SHA1 geprüft.
    public static async Task Run(IReadOnlyCollection<DlItem> items, string stage, IProgress<Progress>? progress,
        CancellationToken ct, int parallel = 12)
    {
        var todo = items.Where(NeedsDownload).DistinctBy(i => i.Path.ToLowerInvariant()).ToList();
        int done = 0, total = todo.Count;
        if (total == 0) return;
        progress?.Report(new Progress(stage, 0, total));

        await Parallel.ForEachAsync(todo, new ParallelOptions { MaxDegreeOfParallelism = parallel, CancellationToken = ct },
            async (item, token) =>
            {
                await DownloadOne(item, token);
                var d = Interlocked.Increment(ref done);
                progress?.Report(new Progress(stage, d, total, System.IO.Path.GetFileName(item.Path)));
            });
    }

    static bool NeedsDownload(DlItem i)
    {
        var f = new FileInfo(i.Path);
        if (!f.Exists) return true;
        if (i.Size > 0) return f.Length != i.Size;
        if (i.Sha1 != null) return !string.Equals(Hash.Sha1File(i.Path), i.Sha1, StringComparison.OrdinalIgnoreCase);
        return false;
    }

    public static async Task DownloadOne(DlItem item, CancellationToken ct)
    {
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(item.Path)!);
        var tmp = item.Path + ".part";
        Exception? last = null;
        for (int attempt = 0; attempt < 3; attempt++)
        {
            try
            {
                using (var res = await Net.Client.GetAsync(item.Url, HttpCompletionOption.ResponseHeadersRead, ct))
                {
                    res.EnsureSuccessStatusCode();
                    await using var src = await res.Content.ReadAsStreamAsync(ct);
                    await using var dst = File.Create(tmp);
                    await src.CopyToAsync(dst, ct);
                }
                if (item.Sha1 != null && !string.Equals(Hash.Sha1File(tmp), item.Sha1, StringComparison.OrdinalIgnoreCase))
                    throw new IOException($"Prüfsumme falsch: {System.IO.Path.GetFileName(item.Path)}");
                File.Move(tmp, item.Path, true);
                return;
            }
            catch (OperationCanceledException) { TryDelete(tmp); throw; }
            catch (Exception e)
            {
                last = e;
                TryDelete(tmp);
                await Task.Delay(500 * (attempt + 1), ct);
            }
        }
        throw new IOException($"Download fehlgeschlagen: {item.Url} ({last?.Message})", last);
    }

    static void TryDelete(string p) { try { File.Delete(p); } catch { } }
}
