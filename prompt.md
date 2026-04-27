export DISPLAY=localhost:0
cargo build
RUST_LOG=gallerypi=info ./target/debug/gallerypi


----------------------------
- Responsive layout
- screenshot tests
- recycler view StandardListView with VecModel 
- If no config file, file picker and save config



---------------

Hardware & Performance
  1. Raspberry pi 4, 1GB RAM
  2. fullscreen. Not sure about desktop or bare-metal, please explain
Media
  3. jpeg, png, webp
  4. mp4
  5. tens of thousands
  6. thumbnail generation is needed

Gallery Screen
  7. Only months with images, scroller shows to the right side, vertical
  8. Grid layout: the grid can be 3 to 6 columns, user can configure this
  9. images should be sorted by time

Image/Video Screen
  10. yes swipe geture
  11. yes zoom/pan support needed
  12. Video controls: play/pause, seek bar and volume

UI Framework — my shortlist for your constraints:
  lets use slint

---------------

/plan I want to create a gallery app targeting linux and raspberry pi OS

Requirements
- App written in rust. Suggest what UI framework to use
- Minimize resource usage
- Inputs: mouse, and touch
- Offline: images are in a folder in the file system, no cloud syncs


Features and views
- Gallery screen that shows images filtered by month and year
 - Scroller that user can use to scroll through the months. scrolling doesnt have animations, instead it jumps immediately to the month the user is picking.
 - Gallery has a button "Reminisce" that will scroll down to a random month/year
- Image and Video Screen that shows an expanded view of the image or video, has video controls, and videos play in loop

Ask questions to clarify requirements and tradeoffs 


-------