// Helpers for C-vs-Rust differential comparison: output buffers, diffing and
// observations for library calls; Run and RunBoth for executable runs. Extend
// or replace as the target needs — this is a starting point, not a fixed
// contract.
#ifndef HARVEST_DIFF_H_
#define HARVEST_DIFF_H_

#include <fcntl.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cerrno>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <map>
#include <optional>
#include <sstream>
#include <string>
#include <vector>

namespace harvest {

// --- Output buffers ---------------------------------------------------------

// Fill pattern for output buffers. Initializing dst to a fixed non-zero pattern
// before each call makes it visible when one side writes a different range or
// leaves bytes untouched (a zero fill hides both, because real output contains
// zeros too).
inline constexpr uint8_t kFillByte = 0xA5;

inline std::vector<uint8_t> FilledBuffer(size_t size) {
  return std::vector<uint8_t>(size, kFillByte);
}

// --- Diffing ----------------------------------------------------------------

// Index of the first byte that differs between two buffers, or std::nullopt if
// they are byte-identical over the compared length. Differing lengths count as
// a difference at the first out-of-range index.
inline std::optional<size_t> FirstDifference(const std::vector<uint8_t>& a,
                                             const std::vector<uint8_t>& b) {
  const size_t n = a.size() < b.size() ? a.size() : b.size();
  for (size_t i = 0; i < n; ++i) {
    if (a[i] != b[i]) {
      return i;
    }
  }
  if (a.size() != b.size()) {
    return n;
  }
  return std::nullopt;
}

// Hex dump of up to `max_bytes` bytes, e.g. "13 bytes: 1b 65 01 00 ... (+5)".
inline std::string HexDump(const std::vector<uint8_t>& data,
                           size_t max_bytes = 64) {
  std::ostringstream os;
  os << data.size() << " bytes:";
  const size_t shown = data.size() < max_bytes ? data.size() : max_bytes;
  char buf[4];
  for (size_t i = 0; i < shown; ++i) {
    std::snprintf(buf, sizeof(buf), " %02x", data[i]);
    os << buf;
  }
  if (data.size() > shown) {
    os << " ... (+" << (data.size() - shown) << ")";
  }
  return os.str();
}

// Streamable comparison detail for use as an EXPECT_EQ message, e.g.
//   EXPECT_EQ(rust, c) << harvest::Explain(c, rust);
// Reports the first differing offset and a hex dump of each side.
inline std::string Explain(const std::vector<uint8_t>& c_out,
                           const std::vector<uint8_t>& rust_out) {
  std::ostringstream os;
  const auto diff = FirstDifference(c_out, rust_out);
  if (diff.has_value()) {
    os << "first difference at byte " << *diff << "\n";
  } else {
    os << "buffers are byte-identical\n";
  }
  os << "  C:    " << HexDump(c_out) << "\n";
  os << "  Rust: " << HexDump(rust_out);
  return os.str();
}

// --- errno capture ----------------------------------------------------------

// Clears errno, runs `call`, and returns (result, errno-after). Use to compare
// the errno each side sets, which is otherwise easy to read after it has been
// clobbered by intervening code.
template <typename Fn>
auto WithErrno(Fn&& call) -> std::pair<decltype(call()), int> {
  errno = 0;
  auto result = call();
  return {result, errno};
}

// --- Normalized observation -------------------------------------------------

// A normalized view of one call's observable result, suitable for EXPECT_EQ.
// Compare only fields that are semantically meaningful — drop pointer values,
// padding, timestamps, and other nondeterministic data before building this.
// Extend with whatever the API actually returns (output length, struct fields,
// errno, ...).
struct Observation {
  int return_value = 0;
  std::vector<uint8_t> output;  // only the meaningful prefix, sized by the call

  bool operator==(const Observation& other) const {
    return return_value == other.return_value && output == other.output;
  }
};

inline std::string Explain(const Observation& c, const Observation& rust) {
  std::ostringstream os;
  os << "return: C=" << c.return_value << " Rust=" << rust.return_value << "\n";
  os << Explain(c.output, rust.output);
  return os.str();
}

// --- Executable runs --------------------------------------------------------

// One run of an executable: argv[1..], stdin, input files written into the
// working directory, and the names of output files to read back afterwards.
// The timeout applies to each process; RunBoth runs two of them per call.
struct Run {
  std::vector<std::string> args;
  std::string stdin_data;
  std::map<std::string, std::string> input_files;
  std::vector<std::string> output_files;
  double timeout_secs = 3;
};

// The observable result of one run. `error` is set when the run could not be
// done at all (missing executable path, fork failure); it never compares equal.
struct RunResult {
  std::string error;
  int exit_code = -1;   // valid when the process exited
  int term_signal = 0;  // nonzero when a signal ended it
  bool timed_out = false;
  std::string out, err;
  std::map<std::string, std::string> files;  // output_files that exist

  bool operator==(const RunResult& o) const {
    return error.empty() && o.error.empty() && exit_code == o.exit_code &&
           term_signal == o.term_signal && timed_out == o.timed_out &&
           out == o.out && err == o.err && files == o.files;
  }
};

inline std::string ReadFile(const std::filesystem::path& p) {
  std::ifstream in(p, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(in), {});
}

// Runs `exe` once with argv[0] = `argv0` in the directory `base`, which it
// empties first: a working directory and a HOME inside `base`, a fixed small
// environment, stdin from a file, and stdout and stderr to files (so large
// output cannot block). The process runs in its own process group; the group
// is killed at the timeout and after the run.
inline RunResult RunExecutable(const std::string& exe, const std::string& argv0,
                               const Run& run, const std::filesystem::path& base) {
  namespace fs = std::filesystem;
  RunResult r;
  for (const auto& entry : fs::directory_iterator(base)) fs::remove_all(entry.path());
  const fs::path cwd = base / "cwd", home = base / "home";
  fs::create_directories(cwd);
  fs::create_directories(home);
  for (const auto& [name, data] : run.input_files) {
    std::ofstream(cwd / name, std::ios::binary) << data;
  }
  std::ofstream(base / "stdin", std::ios::binary) << run.stdin_data;

  std::vector<std::string> argv_s = {argv0};
  argv_s.insert(argv_s.end(), run.args.begin(), run.args.end());
  std::vector<char*> argv;
  for (auto& a : argv_s) argv.push_back(a.data());
  argv.push_back(nullptr);
  std::vector<std::string> env_s = {
      "PATH=/usr/local/bin:/usr/bin:/bin", "HOME=" + home.string(),
      "TMPDIR=" + base.string(), "LC_ALL=C", "TZ=UTC"};
  std::vector<char*> envp;
  for (auto& e : env_s) envp.push_back(e.data());
  envp.push_back(nullptr);

  const pid_t pid = fork();
  if (pid < 0) {
    r.error = std::string("fork: ") + std::strerror(errno);
    return r;
  }
  if (pid == 0) {
    setpgid(0, 0);
    signal(SIGPIPE, SIG_DFL);
    const int in = open((base / "stdin").c_str(), O_RDONLY);
    const int out = open((base / "stdout").c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0600);
    const int err = open((base / "stderr").c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (in < 0 || out < 0 || err < 0 || chdir(cwd.c_str()) != 0) _exit(126);
    dup2(in, 0);
    dup2(out, 1);
    dup2(err, 2);
    execve(exe.c_str(), argv.data(), envp.data());
    _exit(127);
  }
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::duration<double>(run.timeout_secs);
  int status = 0;
  while (waitpid(pid, &status, WNOHANG) == 0) {
    if (std::chrono::steady_clock::now() >= deadline) {
      r.timed_out = true;
      kill(-pid, SIGKILL);
      waitpid(pid, &status, 0);
      break;
    }
    usleep(2000);
  }
  kill(-pid, SIGKILL);  // children the executable left behind
  if (!r.timed_out) {
    if (WIFEXITED(status)) r.exit_code = WEXITSTATUS(status);
    if (WIFSIGNALED(status)) r.term_signal = WTERMSIG(status);
  }
  r.out = ReadFile(base / "stdout");
  r.err = ReadFile(base / "stderr");
  for (const auto& name : run.output_files) {
    if (fs::exists(cwd / name)) r.files[name] = ReadFile(cwd / name);
  }
  return r;
}

// Runs the C reference executable (C_BIN_PATH) and the translated executable
// (RUST_BIN_PATH) on the same Run, one after the other in the same fresh
// directory, so that paths in their output are the same. Both get the same
// argv[0]: the file name of the C executable.
inline std::pair<RunResult, RunResult> RunBoth(const Run& run) {
  namespace fs = std::filesystem;
  auto failed = [](const std::string& error) {
    RunResult r;
    r.error = error;
    return std::pair{r, r};
  };
  const char* c_bin = std::getenv("C_BIN_PATH");
  const char* rust_bin = std::getenv("RUST_BIN_PATH");
  if (c_bin == nullptr || rust_bin == nullptr) {
    return failed("set C_BIN_PATH and RUST_BIN_PATH (see verify_env/README.md)");
  }
  std::string base = (fs::temp_directory_path() / "harvest-run-XXXXXX").string();
  if (mkdtemp(base.data()) == nullptr) {
    return failed(std::string("mkdtemp: ") + std::strerror(errno));
  }
  const std::string argv0 = fs::path(c_bin).filename().string();
  RunResult c = RunExecutable(c_bin, argv0, run, base);
  RunResult rust = RunExecutable(rust_bin, argv0, run, base);
  fs::remove_all(base);
  return {c, rust};
}

inline std::string Explain(const RunResult& c, const RunResult& rust) {
  std::ostringstream os;
  auto side = [&os](const char* name, const RunResult& r) {
    os << name << ": ";
    if (!r.error.empty()) os << "error: " << r.error;
    else if (r.timed_out) os << "timed out";
    else if (r.term_signal != 0) os << "signal " << r.term_signal;
    else os << "exit " << r.exit_code;
    os << ", stdout " << r.out.size() << " bytes, stderr " << r.err.size()
       << " bytes, " << r.files.size() << " output files\n";
  };
  side("C   ", c);
  side("Rust", rust);
  auto first_diff = [&os](const char* what, const std::string& a, const std::string& b) {
    if (a == b) return;
    const auto at = FirstDifference({a.begin(), a.end()}, {b.begin(), b.end()});
    const size_t from = *at > 40 ? *at - 40 : 0;
    os << what << " differs at byte " << *at << "\n  C:    "
       << a.substr(from, 120) << "\n  Rust: " << b.substr(from, 120) << "\n";
  };
  first_diff("stdout", c.out, rust.out);
  first_diff("stderr", c.err, rust.err);
  for (const auto& [name, data] : c.files) {
    auto it = rust.files.find(name);
    first_diff(("file " + name).c_str(), data, it == rust.files.end() ? "" : it->second);
  }
  return os.str();
}

}  // namespace harvest

#endif  // HARVEST_DIFF_H_
