use crate::pb::ProgressBar;
use crate::tty::move_cursor_up;
use crossbeam::channel::{self, Receiver, Sender};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::{Result, Stdout, Write};
use std::str::from_utf8;
use std::sync::Arc;

// StateMessage is the message format used to communicate
// between MultiBar and its bars
enum StateMessage {
    ProgressMessage(String, usize),
    BarFinished(usize),
    Finish,
    Reprint,
}

enum Line {
    Flat(String),
    Progress(String),
}

pub struct MultiBarPrinter<T: Write> {
    receiver: Receiver<StateMessage>,
    handle: T,

    lines: Arc<Mutex<VecDeque<Line>>>,
    offset: usize,
    last_len: usize,
}

fn print_lines<T: Write>(handle: &mut T, lines: &parking_lot::MutexGuard<'_, VecDeque<Line>>) {
    let len = lines.len();

    // print each line
    let mut out = String::new();
    let mut first = true;
    for line in lines.iter() {
        match line {
            Line::Flat(message) | Line::Progress(message) => {
                if first {
                    out += &move_cursor_up(len);
                    first = false;
                }

                out.push_str(&format!("{}\n", message));
            }
        }
    }
    printfl!(handle, "{}", out);
}

impl<T: Write> MultiBarPrinter<T> {
    // returns true if more bars could be added later
    // false if all MultiBarSenders have been dropped
    pub fn listen(&mut self) -> bool {
        let mut changed = true;

        loop {
            // receive message
            let msg = self.receiver.recv().unwrap();
            let mut lines = self.lines.lock();
            let len = lines.len();

            // when new lines are added, print newlines to shift scrollback up
            if len > self.last_len {
                for _ in 0..(len - self.last_len) {
                    printfl!(self.handle, "\n");
                }
            }
            self.last_len = len;

            if changed {
                #[allow(unused_assignments)] // bug
                {
                    changed = false;
                }

                // there was a change, so reprint
                print_lines(&mut self.handle, &lines);

                // pop the first line if it is useless
                loop {
                    if lines
                        .get(0)
                        .map(|line| match line {
                            Line::Flat(_) => true,
                            _ => false,
                        })
                        .unwrap_or(false)
                    {
                        self.offset += 1;
                        lines.pop_front();
                    } else {
                        break;
                    }
                }

                // if we have no ProgressBar Lines
                // stop listening
                if lines
                    .iter()
                    .filter(|line| match line {
                        Line::Progress(_) => true,
                        _ => false,
                    })
                    .count()
                    == 0
                {
                    // re-print newlines when we start listening again
                    self.last_len = 0;
                    return true;
                }
            }

            match msg {
                StateMessage::Reprint => {
                    changed = true;
                }
                StateMessage::ProgressMessage(message, level) => {
                    if let Line::Flat(_) = lines[level - self.offset] {
                        debug_assert!(false, "ProcessMessage on a Flat Line!");
                    }

                    lines[level - self.offset] = Line::Progress(message);

                    changed = true;
                }
                StateMessage::BarFinished(level) => {
                    debug_assert!(level >= self.offset, "{} >= {}", level, self.offset);
                    debug_assert!(
                        lines.get(level - self.offset).is_some(),
                        "lines[{} - {}] is not some",
                        level,
                        self.offset
                    );
                    if let Line::Flat(_) = lines[level - self.offset] {
                        debug_assert!(false, "BarFinished on a Flat Line!");
                    }

                    let message = match &lines[level - self.offset] {
                        Line::Flat(message) | Line::Progress(message) => message.to_owned(),
                    };

                    // mark line as "flat" since it will not receive anymore updates
                    lines[level - self.offset] = Line::Flat(message);

                    changed = true;
                }
                StateMessage::Finish => {
                    // all references to this printer have been lost
                    return false;
                }
            }
        }
    }
}

/// A clonable struct that acts the same as `MultiBar`, minus `listen()`
pub struct MultiBarSender {
    nlines: Arc<Mutex<usize>>,
    lines: Arc<Mutex<VecDeque<Line>>>,
    sender: Sender<StateMessage>,
    refs: Arc<()>,
}

impl MultiBarSender {
    /// println used to add text lines between the bars.
    /// for example: you could add a header to your application,
    /// or text separators between bars.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    /// mb.println("Application header:");
    ///
    /// # let count = 250;
    /// let mut p1 = mb.create_bar(count);
    /// // ...
    ///
    /// mb.println("Text line between bar1 and bar2");
    ///
    /// let mut p2 = mb.create_bar(count);
    /// // ...
    ///
    /// mb.println("Text line between bar2 and bar3");
    ///
    /// // ...
    /// // ...
    /// mb.listen();
    /// ```
    pub fn println(&mut self, s: &str) {
        self.lines.lock().push_back(Line::Flat(s.to_owned()));
        *self.nlines.lock() += 1;

        self.sender.send(StateMessage::Reprint).unwrap();
    }

    /// create_bar creates new `ProgressBar` with `Pipe` as the writer.
    ///
    /// The ordering of the method calls is important. it means that in
    /// the first call, you get a progress bar in level 1, in the 2nd call,
    /// you get a progress bar in level 2, and so on.
    ///
    /// ProgressBar that finish its work, must call `finish()` (or `finish_print`)
    /// to notify the `MultiBar` about it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    /// # let (count1, count2, count3) = (250, 62500, 15625000);
    ///
    /// // progress bar in level 1
    /// let mut p1 = mb.create_bar(count1);
    /// // ...
    ///
    /// // progress bar in level 2
    /// let mut p2 = mb.create_bar(count2);
    /// // ...
    ///
    /// // progress bar in level 3
    /// let mut p3 = mb.create_bar(count3);
    ///
    /// // ...
    /// mb.listen();
    /// ```
    pub fn create_bar(&mut self, total: u64) -> ProgressBar<Pipe> {
        self.lines.lock().push_back(Line::Progress("".to_owned()));

        let mut nlines = self.nlines.lock();
        let level = *nlines;
        *nlines += 1;

        self.sender.send(StateMessage::Reprint).unwrap();

        let mut p = ProgressBar::on(
            Pipe {
                level,
                sender: self.sender.clone(),
            },
            total,
        );
        p.is_multibar = true;
        p.add(0);
        p
    }
}

impl Clone for MultiBarSender {
    fn clone(&self) -> Self {
        MultiBarSender {
            nlines: self.nlines.clone(),
            lines: self.lines.clone(),
            sender: self.sender.clone(),
            refs: self.refs.clone(),
        }
    }
}

impl Drop for MultiBarSender {
    fn drop(&mut self) {
        if Arc::strong_count(&self.refs) == 2 {
            // we are the last cloned sender, other than the child of mb
            self.sender.send(StateMessage::Finish).unwrap();
        }
    }
}

pub struct MultiBar<T: Write> {
    mbs: MultiBarSender,
    mbp: MultiBarPrinter<T>,
}

impl MultiBar<Stdout> {
    /// Create a new MultiBar with stdout as a writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::thread;
    /// use pbr::MultiBar;
    /// use std::time::Duration;
    ///
    /// let mut mb = MultiBar::new();
    /// mb.println("Application header:");
    ///
    /// # let count = 250;
    /// let mut p1 = mb.create_bar(count);
    /// let _ = thread::spawn(move || {
    ///     for _ in 0..count {
    ///         p1.inc();
    ///         thread::sleep(Duration::from_millis(100));
    ///     }
    ///     // notify the multibar that this bar finished.
    ///     p1.finish();
    /// });
    ///
    /// mb.println("add a separator between the two bars");
    ///
    /// let mut p2 = mb.create_bar(count * 2);
    /// let _ = thread::spawn(move || {
    ///     for _ in 0..count * 2 {
    ///         p2.inc();
    ///         thread::sleep(Duration::from_millis(100));
    ///     }
    ///     // notify the multibar that this bar finished.
    ///     p2.finish();
    /// });
    ///
    /// // start listen to all bars changes.
    /// // this is a blocking operation, until all bars will finish.
    /// // to ignore blocking, you can run it in a different thread.
    /// mb.listen();
    /// ```
    pub fn new() -> MultiBar<Stdout> {
        MultiBar::on(::std::io::stdout())
    }
}

impl<T: Write> MultiBar<T> {
    /// Create a new MultiBar with an arbitrary writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    /// use std::io::stderr;
    ///
    /// let mut mb = MultiBar::on(stderr());
    /// // ...
    /// // see full example in `MultiBar::new`
    /// // ...
    /// ```
    pub fn on(handle: T) -> MultiBar<T> {
        let (sender, receiver) = channel::unbounded();
        let lines = Arc::new(Mutex::new(VecDeque::new()));

        MultiBar {
            mbs: MultiBarSender {
                nlines: Arc::new(Mutex::new(0)),
                lines: lines.clone(),
                sender,
                refs: Arc::new(()),
            },
            mbp: MultiBarPrinter {
                receiver,
                lines,
                handle,
                offset: 0,
                last_len: 0,
            },
        }
    }

    /// listen start listen to all bars changes.
    /// returns false if all senders were cleaned up
    ///
    /// `ProgressBar` that finish its work, must call `finish()` (or `finish_print`)
    /// to notify the `MultiBar` about it.
    ///
    /// This is a blocking operation and blocks until all bars will
    /// finish.
    /// To ignore blocking, you can run it in a different thread.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::thread;
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    ///
    /// // ...
    /// // create some bars here
    /// // ...
    ///
    /// thread::spawn(move || {
    ///     mb.listen();
    ///     println!("all bars done!");
    /// });
    ///
    /// // ...
    /// ```
    pub fn listen(&mut self) -> bool {
        if self.mbp.lines.lock().is_empty() && Arc::strong_count(&self.mbs.refs) == 1 {
            // if there are lines to print, and no senders, do nothing
            return false;
        }
        self.mbp.listen()
    }
}

impl Default for MultiBar<Stdout> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Write> std::ops::Deref for MultiBar<T> {
    type Target = MultiBarSender;

    fn deref(&self) -> &MultiBarSender {
        &self.mbs
    }
}

impl<T: Write> std::ops::DerefMut for MultiBar<T> {
    fn deref_mut(&mut self) -> &mut MultiBarSender {
        &mut self.mbs
    }
}

pub struct Pipe {
    level: usize,
    sender: Sender<StateMessage>,
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let s = from_utf8(buf).unwrap().to_owned();

        // finish method emits empty string
        let msg = if s == "" {
            StateMessage::BarFinished(self.level)
        } else {
            StateMessage::ProgressMessage(s, self.level)
        };

        self.sender.send(msg).unwrap();

        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
