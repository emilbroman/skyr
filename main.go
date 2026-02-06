package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strings"

	kivik "github.com/go-kivik/kivik/v4"
	_ "github.com/go-kivik/kivik/v4/couchdb"

	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp/capability"
	"github.com/go-git/go-git/v5/plumbing/storer"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-git/go-git/v5/plumbing/transport/server"

	"github.com/gliderlabs/ssh"
)

type nopCloseReader struct {
	io.Reader
}

func (nopCloseReader) Close() error { return nil }

func pktLine(w io.Writer, content string) error {
	buf := fmt.Appendf(nil, "0000%s\n", content)

	pktLen := uint16(len(buf))
	hex.Encode(buf, binary.BigEndian.AppendUint16(nil, pktLen))

	slog.Debug("writing pkt-line", slog.String("line", string(buf)))

	_, err := w.Write(buf)
	return err
}

func errorLinef(w io.Writer, format string, a ...any) {
	pktLine(w, fmt.Sprintf("ERR %s", fmt.Sprintf(format, a...)))
}

func main() {
	slog.SetLogLoggerLevel(slog.LevelDebug)

	client, err := kivik.New("couch", "http://skyr:password123@localhost:5984/")
	if err != nil {
		slog.Error("failed to connect to db", "", err)
		os.Exit(1)
	}

	srv := server.NewServer(CouchDBRepoLoader{dbClient: client, ctx: context.Background()})

	ssh.Handle(func(s ssh.Session) {
		defer s.Exit(0)

		cmd := s.Command()[0]
		args := s.Command()[1:]

		switch cmd {
		case "git-receive-pack":
			endpoint, err := transport.NewEndpoint(fmt.Sprintf("git://%s:@localhost%s", s.User(), args[0]))

			if err != nil {
				errorLinef(s, "Invalid repo: %s", args[0])
				return
			}

			sess, err := srv.NewReceivePackSession(endpoint, nil)
			if err != nil {
				errorLinef(s, "Failed to open session: %v", err)
				return
			}

			advrefs, err := sess.AdvertisedReferences()
			if err != nil {
				errorLinef(s, "Failed to negotiate references: %v", err)
				return
			}

			err = advrefs.Encode(s)
			if err != nil {
				errorLinef(s, "Failed to advertise references: %v", err)
				return
			}

			caps := capability.NewList()

			report := false

			if advrefs.Capabilities.Supports(capability.ReportStatus) {
				report = true
				caps.Add(capability.ReportStatus)
			}

			r := packp.NewReferenceUpdateRequestFromCapabilities(caps)

			err = r.Decode(nopCloseReader{s})
			if err != nil {
				errorLinef(s, "Failed to decode request: %v", err)
				return
			}

			status, err := sess.ReceivePack(s.Context(), r)
			if err != nil {
				errorLinef(s, "Failed to receive pack: %v", err)
				return
			}

			if report {
				err = status.Encode(s)
				if err != nil {
					slog.Error("bad state", slog.Any("err", err))
					return
				}
			}

		// case "git-upload-pack":

		default:
			errorLinef(s, "Unsupported command: %s", cmd)
			return
		}
	})

	err = ssh.ListenAndServe(":2222", nil,
		ssh.HostKeyFile("host.pem"),
		ssh.PublicKeyAuth(func(ctx ssh.Context, key ssh.PublicKey) bool {
			slog.Info("TODO: Auth",
				slog.String("user", ctx.User()),
				slog.String("pubkey", hex.EncodeToString(key.Marshal())),
			)
			return true
		}),
	)
	if err != nil {
		slog.Error("failed to serve", "", err)
		os.Exit(1)
	}
}

func createDatabaseIfNotExists(c *kivik.Client, name string) (*kivik.DB, error) {
	err := c.CreateDB(context.Background(), name)

	if err != nil && !strings.Contains(err.Error(), "already exists") {
		return nil, err
	}

	return c.DB(name), nil
}

type CouchDBRepoLoader struct {
	ctx      context.Context
	dbClient *kivik.Client
}

func (c CouchDBRepoLoader) Load(ep *transport.Endpoint) (storer.Storer, error) {
	slog.Debug("opening repo", slog.String("path", ep.Path), slog.String("user", ep.User))

	objDB, err := createDatabaseIfNotExists(c.dbClient, fmt.Sprintf("repo/objs%s", ep.Path))
	if err != nil {
		return nil, err
	}

	refDB, err := createDatabaseIfNotExists(c.dbClient, fmt.Sprintf("repo/refs%s", ep.Path))
	if err != nil {
		return nil, err
	}

	return NewCouchDBStorer(c.ctx, objDB, refDB, ep), nil
}

type CouchDBStorer struct {
	ctx context.Context

	objDB *kivik.DB
	refDB *kivik.DB

	ep *transport.Endpoint
}

func NewCouchDBStorer(ctx context.Context, objDB *kivik.DB, refDB *kivik.DB, ep *transport.Endpoint) CouchDBStorer {
	return CouchDBStorer{ctx, objDB, refDB, ep}
}

type RefDoc struct {
	ID  string `json:"_id,omitempty"`
	Rev string `json:"_rev,omitempty"`

	Type           plumbing.ReferenceType `json:"type"`
	Name           plumbing.ReferenceName `json:"name"`
	Hash           string                 `json:"hash,omitempty"`
	SymbolicTarget plumbing.ReferenceName `json:"symbolic_target,omitempty"`
}

func (r *RefDoc) ToReference() *plumbing.Reference {
	if r.Hash != "" {
		return plumbing.NewHashReference(r.Name, plumbing.NewHash(r.Hash))
	}
	return plumbing.NewSymbolicReference(r.Name, r.SymbolicTarget)
}

type scanable interface {
	Err() error
	ScanDoc(any) error
}

func decodeDoc[T any](doc scanable) (*T, error) {
	if doc.Err() != nil {
		if strings.Contains(doc.Err().Error(), "Not Found") {
			return nil, nil
		}

		return nil, doc.Err()
	}

	var d T
	err := doc.ScanDoc(&d)
	if err != nil {
		return nil, err
	}

	return &d, nil
}

// CheckAndSetReference implements [storer.ReferenceStorer].
func (c CouchDBStorer) CheckAndSetReference(new *plumbing.Reference, old *plumbing.Reference) error {
	if old.Name() != new.Name() {
		return fmt.Errorf("cannot set %v over %v", new.Name(), old.Name())
	}

	docID := old.Name().String()

	existing, err := decodeDoc[RefDoc](c.refDB.Get(c.ctx, docID))
	if err != nil {
		if strings.Contains(err.Error(), "Not Found") {
			return plumbing.ErrReferenceNotFound
		}
		return err
	}

	if old != nil && existing != nil {
		if old.Hash() != plumbing.NewHash(existing.Hash) {
			return fmt.Errorf("%v is not up-to-date", old.Name())
		}
	}

	newDoc := &RefDoc{
		ID:             docID,
		Rev:            existing.Rev,
		Type:           new.Type(),
		Name:           new.Name(),
		Hash:           new.Hash().String(),
		SymbolicTarget: new.Target(),
	}

	_, err = c.refDB.Put(context.Background(), existing.ID, newDoc)

	return err
}

// CountLooseRefs implements [storer.ReferenceStorer].
func (c CouchDBStorer) CountLooseRefs() (int, error) {
	return 0, nil
}

// IterReferences implements [storer.ReferenceStorer].
func (c CouchDBStorer) IterReferences() (storer.ReferenceIter, error) {
	slog.Info("listing refs")
	set := c.refDB.AllDocs(c.ctx, kivik.IncludeDocs())
	if set.Err() != nil {
		return nil, set.Err()
	}
	return couchReferenceIter{set}, nil
}

type couchReferenceIter struct {
	set *kivik.ResultSet
}

// Close implements [storer.ReferenceIter].
func (c couchReferenceIter) Close() {
	if err := c.set.Close(); err != nil {
		slog.Error("error while closing result set", slog.Any("err", err))
	}
}

// ForEach implements [storer.ReferenceIter].
func (c couchReferenceIter) ForEach(f func(*plumbing.Reference) error) error {
	for c.set.Next() {
		doc, err := decodeDoc[RefDoc](c.set)
		if err != nil {
			c.set.Close()
			return err
		}

		err = f(doc.ToReference())
		if err != nil {
			c.set.Close()
			return err
		}
	}

	return nil
}

// Next implements [storer.ReferenceIter].
func (c couchReferenceIter) Next() (*plumbing.Reference, error) {
	if !c.set.Next() {
		return nil, nil
	}

	doc, err := decodeDoc[RefDoc](c.set)
	if err != nil {
		c.set.Close()
		return nil, err
	}

	return doc.ToReference(), nil
}

// PackRefs implements [storer.ReferenceStorer].
func (c CouchDBStorer) PackRefs() error {
	slog.Error("ref packing not implemented")
	return nil
}

// Reference implements [storer.ReferenceStorer].
func (c CouchDBStorer) Reference(name plumbing.ReferenceName) (*plumbing.Reference, error) {
	doc, err := decodeDoc[RefDoc](c.refDB.Get(c.ctx, name.String()))
	if err != nil {
		return nil, err
	}
	if doc == nil {
		return nil, plumbing.ErrReferenceNotFound
	}
	return doc.ToReference(), nil
}

// RemoveReference implements [storer.ReferenceStorer].
func (c CouchDBStorer) RemoveReference(name plumbing.ReferenceName) error {
	slog.Info("removing reference", slog.String("ep", c.ep.String()), slog.String("ref", name.String()))
	_, err := c.refDB.Delete(c.ctx, name.String(), "")
	if err != nil {
		if strings.Contains(err.Error(), "Not Found") {
			return plumbing.ErrReferenceNotFound
		}
		return err
	}
	return nil
}

// SetReference implements [storer.ReferenceStorer].
func (c CouchDBStorer) SetReference(ref *plumbing.Reference) error {
	docID := ref.Name().String()

	rev, err := c.refDB.GetRev(c.ctx, docID)
	if err != nil && !strings.Contains(err.Error(), "Not Found") {
		return err
	}

	slog.Info("setting reference", slog.String("ep", c.ep.String()), slog.String("ref", ref.Name().String()), slog.String("h", ref.Hash().String()), slog.String("rev", rev))

	d := &RefDoc{
		ID:             docID,
		Rev:            rev,
		Type:           ref.Type(),
		Name:           ref.Name(),
		Hash:           ref.Hash().String(),
		SymbolicTarget: ref.Target(),
	}

	_, err = c.refDB.Put(context.Background(), docID, d)
	return err
}

// AddAlternate implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) AddAlternate(remote string) error {
	return errors.ErrUnsupported
}

type ObjDoc struct {
	ID  string `json:"_id,omitempty"`
	Rev string `json:"_rev,omitempty"`

	Type_ plumbing.ObjectType `json:"type"`
	Size_ int64               `json:"size"`
	Data  []byte              `json:"data"`

	open bool
}

func (d *ObjDoc) Hash() plumbing.Hash {
	return plumbing.NewHash(d.ID)
}

func (d *ObjDoc) Type() plumbing.ObjectType {
	return d.Type_
}

func (d *ObjDoc) SetType(t plumbing.ObjectType) {
	d.Type_ = t
}

func (d *ObjDoc) Size() int64 {
	return d.Size_
}

func (d *ObjDoc) SetSize(s int64) {
	d.Size_ = s
}

func (d *ObjDoc) Reader() (io.ReadCloser, error) {
	return newBlobBuffer(d), nil
}

func (d *ObjDoc) Writer() (io.WriteCloser, error) {
	return newBlobBuffer(d), nil
}

type blobBuffer struct {
	bytes.Buffer
	doc *ObjDoc
}

func newBlobBuffer(doc *ObjDoc) *blobBuffer {
	doc.open = true
	return &blobBuffer{
		*bytes.NewBuffer(doc.Data),
		doc,
	}
}

func (b *blobBuffer) Close() error {
	b.doc.open = false
	return nil
}

// EncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) EncodedObject(t plumbing.ObjectType, h plumbing.Hash) (out plumbing.EncodedObject, err error) {
	d, err := decodeDoc[ObjDoc](c.objDB.Get(c.ctx, h.String()))
	if err != nil {
		return nil, err
	}
	if t != plumbing.AnyObject && d.Type() != t {
		return nil, plumbing.ErrInvalidType
	}
	return d, nil
}

// EncodedObjectSize implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) EncodedObjectSize(h plumbing.Hash) (int64, error) {
	d, err := decodeDoc[ObjDoc](c.objDB.Get(c.ctx, h.String()))
	if err != nil {
		return 0, err
	}
	return d.Size(), nil
}

// HasEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) HasEncodedObject(h plumbing.Hash) error {
	_, err := c.objDB.GetRev(c.ctx, h.String())
	if err != nil {
		return err
	}
	return nil
}

// IterEncodedObjects implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) IterEncodedObjects(t plumbing.ObjectType) (storer.EncodedObjectIter, error) {
	set := c.objDB.Find(c.ctx, map[string]any{"selector": map[string]any{
		"type": t,
	}})

	return couchObjIter{set}, nil
}

// NewEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) NewEncodedObject() plumbing.EncodedObject {
	return &ObjDoc{}
}

// SetEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) SetEncodedObject(o plumbing.EncodedObject) (plumbing.Hash, error) {
	slog.Info("setting obj", slog.String("ep", c.ep.String()), slog.String("ot", o.Type().String()), slog.String("h", o.Hash().String()))
	h := o.Hash()

	docID := h.String()
	rev, err := c.objDB.GetRev(c.ctx, docID)
	if err != nil && !strings.Contains(err.Error(), "Not Found") {
		return h, err
	}

	r, err := o.Reader()
	if err != nil {
		return h, err
	}
	defer r.Close()
	data, err := io.ReadAll(r)

	doc := &ObjDoc{
		ID:    h.String(),
		Rev:   rev,
		Type_: o.Type(),
		Size_: o.Size(),
		Data:  data,
	}

	enc, _ := json.Marshal(doc)
	slog.Info("put record",
		slog.String("docid", docID),
		slog.Any("doc", enc),
		slog.Any("ID", h.String()),
		slog.Any("Rev", rev),
		slog.Any("typ", o.Type()),
		slog.Any("size", o.Size()),
		slog.Any("data", data),
		slog.Any("open", false),
	)

	_, err = c.objDB.Put(c.ctx, docID, doc)

	return h, err
}

type couchObjIter struct {
	set *kivik.ResultSet
}

// Close implements [storer.EncodedObjectIter].
func (c couchObjIter) Close() {
	if err := c.set.Close(); err != nil {
		slog.Error("error while closing result set", slog.Any("err", err))
	}
}

// ForEach implements [storer.EncodedObjectIter].
func (c couchObjIter) ForEach(f func(plumbing.EncodedObject) error) error {
	for {
		doc, err := decodeDoc[ObjDoc](c.set)
		if err != nil {
			c.set.Close()
			return err
		}

		err = f(doc)
		if err != nil {
			c.set.Close()
			return err
		}

		if !c.set.Next() {
			return nil
		}
	}
}

// Next implements [storer.EncodedObjectIter].
func (c couchObjIter) Next() (plumbing.EncodedObject, error) {
	if !c.set.Next() {
		return nil, nil
	}

	doc, err := decodeDoc[ObjDoc](c.set)
	if err != nil {
		c.set.Close()
		return nil, err
	}

	return doc, nil
}
