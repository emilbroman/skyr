package confsrv

import (
	"bytes"
	"io"

	"github.com/go-git/go-git/v5/plumbing"
)

type ObjDoc struct {
	ID  string  `json:"_id,omitempty"`
	Rev string  `json:"_rev,omitempty"`
	Obj ObjBlob `json:"obj"`
}

type ObjBlob struct {
	Type plumbing.ObjectType `json:"type"`
	Size int64               `json:"size"`
	Data []byte              `json:"data"`
}

// Hash implements [plumbing.EncodedObject]
func (d *ObjDoc) Hash() plumbing.Hash {
	return plumbing.NewHash(d.ID)
}

// Type implements [plumbing.EncodedObject]
func (d *ObjDoc) Type() plumbing.ObjectType {
	return d.Obj.Type
}

// SetType implements [plumbing.EncodedObject]
func (d *ObjDoc) SetType(t plumbing.ObjectType) {
	d.Obj.Type = t
}

// Size implements [plumbing.EncodedObject]
func (d *ObjDoc) Size() int64 {
	return d.Obj.Size
}

// SetSize implements [plumbing.EncodedObject]
func (d *ObjDoc) SetSize(s int64) {
	d.Obj.Size = s
}

// Reader implements [plumbing.EncodedObject]
func (d *ObjDoc) Reader() (io.ReadCloser, error) {
	return NewNopReadCloser(bytes.NewBuffer(d.Obj.Data)), nil
}

// Writer implements [plumbing.EncodedObject]
func (d *ObjDoc) Writer() (io.WriteCloser, error) {
	return NewNopWriteCloser(bytes.NewBuffer(d.Obj.Data)), nil
}
