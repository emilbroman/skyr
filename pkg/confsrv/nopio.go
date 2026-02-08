package confsrv

import "io"

type nopReadCloser struct {
	io.Reader
}

func (nopReadCloser) Close() error {
	return nil
}

func NewNopReadCloser(r io.Reader) io.ReadCloser {
	return nopReadCloser{r}
}

type nopWriteCloser struct {
	io.Writer
}

func (nopWriteCloser) Close() error {
	return nil
}

func NewNopWriteCloser(r io.Writer) io.WriteCloser {
	return nopWriteCloser{r}
}
