# better ia downloader

A better [internet archive](https://archive.org) downloader that is natively "multi-threaded" and async.

```shell
Usage: better-ia-downloader [OPTIONS] <RESOURCE_NAME>

Arguments:
  <RESOURCE_NAME>  Resource name to download files from IA

Options:
      --sd                     Skip duplicate files based on file signatures
      --rd                     Remove any files marked as derivatives by IA
  -t <THREADS>                 Number of threads to use for downloading [default: 2]
  -d <DOWNLOAD_DIRECTORY>      Directory to save downloads to By default downloads to a directory of the resource name [default: auto]
  -r <RETRIES>                 Number of retries to attempt on failed downloads [default: 3]
  -f <FILTER>                  Regex to mach against file names to determine what to download
      --no-check               Skip validation of file hashes and sizes to prevent redownloading already downloaded files
  -h, --help                   Print help
```