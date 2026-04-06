//! Text Adventure Game - A fun easter egg!
//!
//! A simple interactive fiction game accessible via `/game` command.
//! Navigate rooms, collect items, solve puzzles.

use std::collections::HashMap;

/// Game direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl Direction {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "n" | "north" => Some(Direction::North),
            "s" | "south" => Some(Direction::South),
            "e" | "east" => Some(Direction::East),
            "w" | "west" => Some(Direction::West),
            "u" | "up" => Some(Direction::Up),
            "d" | "down" => Some(Direction::Down),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Direction::North => "north",
            Direction::South => "south",
            Direction::East => "east",
            Direction::West => "west",
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }

    pub fn opposite(&self) -> Self {
        match self {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::East => Direction::West,
            Direction::West => Direction::East,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }
}

/// Game room
#[derive(Debug, Clone)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub description: String,
    pub exits: HashMap<Direction, String>,
    pub items: Vec<Item>,
}

/// Game item
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub description: String,
    pub takeable: bool,
    pub usable: bool,
}

/// Player state
#[derive(Debug, Clone)]
pub struct Player {
    pub current_room: String,
    pub inventory: Vec<Item>,
}

/// Game state
#[derive(Debug, Clone)]
pub struct Game {
    pub rooms: HashMap<String, Room>,
    pub player: Player,
    pub game_over: bool,
    pub won: bool,
    pub moves: u32,
    pub score: i32,
}

impl Game {
    /// Create a new game with default world
    pub fn new() -> Self {
        let mut game = Self {
            rooms: HashMap::new(),
            player: Player {
                current_room: "start".to_string(),
                inventory: Vec::new(),
            },
            game_over: false,
            won: false,
            moves: 0,
            score: 0,
        };

        game.setup_world();
        game
    }

    /// Setup the game world
    fn setup_world(&mut self) {
        // Starting room
        let start = Room {
            id: "start".to_string(),
            name: "Entrance Hall".to_string(),
            description: "You stand in a grand entrance hall. Dust motes dance in shafts of light from high windows. A corridor leads north, and there's a door to the east.".to_string(),
            exits: {
                let mut m = HashMap::new();
                m.insert(Direction::North, "corridor".to_string());
                m.insert(Direction::East, "library".to_string());
                m
            },
            items: vec![
                Item {
                    id: "key".to_string(),
                    name: "brass key".to_string(),
                    description: "An old brass key, slightly tarnished.".to_string(),
                    takeable: true,
                    usable: true,
                },
            ],
        };

        // Corridor
        let corridor = Room {
            id: "corridor".to_string(),
            name: "Long Corridor".to_string(),
            description: "A long corridor stretches before you. Paintings line the walls, their subjects shrouded in shadow. You can go south or east.".to_string(),
            exits: {
                let mut m = HashMap::new();
                m.insert(Direction::South, "start".to_string());
                m.insert(Direction::East, "treasure".to_string());
                m
            },
            items: vec![],
        };

        // Library
        let library = Room {
            id: "library".to_string(),
            name: "Old Library".to_string(),
            description: "Books line the walls from floor to ceiling. The smell of old paper fills the air. A ladder leads up to a loft. You can go west or up.".to_string(),
            exits: {
                let mut m = HashMap::new();
                m.insert(Direction::West, "start".to_string());
                m.insert(Direction::Up, "loft".to_string());
                m
            },
            items: vec![
                Item {
                    id: "book".to_string(),
                    name: "ancient book".to_string(),
                    description: "A heavy leather-bound book with strange symbols.".to_string(),
                    takeable: true,
                    usable: false,
                },
            ],
        };

        // Loft
        let loft = Room {
            id: "loft".to_string(),
            name: "Dusty Loft".to_string(),
            description: "A small dusty loft beneath the roof. Cobwebs hang from the rafters. You can only go down.".to_string(),
            exits: {
                let mut m = HashMap::new();
                m.insert(Direction::Down, "library".to_string());
                m
            },
            items: vec![
                Item {
                    id: "lantern".to_string(),
                    name: "oil lantern".to_string(),
                    description: "A brass oil lantern. It's full and ready to use.".to_string(),
                    takeable: true,
                    usable: true,
                },
            ],
        };

        // Treasure room
        let treasure = Room {
            id: "treasure".to_string(),
            name: "Treasure Chamber".to_string(),
            description: "A magnificent chamber with a large chest in the center. The chest is locked with a heavy padlock. You can go west.".to_string(),
            exits: {
                let mut m = HashMap::new();
                m.insert(Direction::West, "corridor".to_string());
                m
            },
            items: vec![],
        };

        self.rooms.insert("start".to_string(), start);
        self.rooms.insert("corridor".to_string(), corridor);
        self.rooms.insert("library".to_string(), library);
        self.rooms.insert("loft".to_string(), loft);
        self.rooms.insert("treasure".to_string(), treasure);
    }

    /// Process a command
    pub fn process_command(&mut self, input: &str) -> String {
        if self.game_over {
            return "Game over! Type 'restart' to play again.".to_string();
        }

        let input_lower = input.trim().to_lowercase();
        let parts: Vec<&str> = input_lower.split_whitespace().collect();
        if parts.is_empty() {
            return "What would you like to do?".to_string();
        }

        let command = parts[0];
        let args = &parts[1..];

        let result = match command {
            "look" | "l" => self.look(),
            "north" | "n" => self.go(Direction::North),
            "south" | "s" => self.go(Direction::South),
            "east" | "e" => self.go(Direction::East),
            "west" | "w" => self.go(Direction::West),
            "up" | "u" => self.go(Direction::Up),
            "down" | "d" => self.go(Direction::Down),
            "go" => {
                if let Some(dir) = args.get(0).and_then(|d| Direction::from_str(d)) {
                    self.go(dir)
                } else {
                    "Go where?".to_string()
                }
            }
            "inventory" | "i" => self.inventory(),
            "take" | "get" => {
                if let Some(item) = args.get(0) {
                    self.take(item)
                } else {
                    "Take what?".to_string()
                }
            }
            "drop" => {
                if let Some(item) = args.get(0) {
                    self.drop(item)
                } else {
                    "Drop what?".to_string()
                }
            }
            "examine" | "x" => {
                if let Some(item) = args.get(0) {
                    self.examine(item)
                } else {
                    "Examine what?".to_string()
                }
            }
            "use" => {
                if let Some(item) = args.get(0) {
                    self.use_item(item)
                } else {
                    "Use what?".to_string()
                }
            }
            "help" | "?" => self.help(),
            "quit" | "q" => {
                self.game_over = true;
                "Thanks for playing! Final score: {}".to_string()
            }
            "restart" => {
                *self = Game::new();
                self.look()
            }
            "score" => format!("Score: {} | Moves: {}", self.score, self.moves),
            _ => "I don't understand that command. Type 'help' for instructions.".to_string(),
        };

        if command != "score" && command != "help" && command != "look" {
            self.moves += 1;
        }

        // Check win condition
        if !self.game_over && self.won {
            self.game_over = true;
            return format!("{}\n\n🎉 CONGRATULATIONS! 🎉\nYou've unlocked the treasure!\nFinal Score: {} | Moves: {}", result, self.score + 100, self.moves);
        }

        result
    }

    /// Look around current room
    pub fn look(&self) -> String {
        let room = self.rooms.get(&self.player.current_room).unwrap();
        
        let mut output = format!("\n{}\n{}", room.name, "=".repeat(room.name.len()));
        output.push_str(&format!("\n\n{}", room.description));

        // Show exits
        if !room.exits.is_empty() {
            let exit_names: Vec<_> = room.exits.keys().map(|d| d.name()).collect();
            output.push_str(&format!("\n\nExits: {}", exit_names.join(", ")));
        }

        // Show items
        if !room.items.is_empty() {
            let item_names: Vec<_> = room.items.iter().map(|i| i.name.clone()).collect();
            output.push_str(&format!("\n\nYou see: {}", item_names.join(", ")));
        }

        output
    }

    /// Go in a direction
    fn go(&mut self, direction: Direction) -> String {
        let room = self.rooms.get(&self.player.current_room).unwrap();
        
        if let Some(next_room_id) = room.exits.get(&direction) {
            self.player.current_room = next_room_id.clone();
            self.score += 1;
            self.look()
        } else {
            format!("You can't go {}.", direction.name())
        }
    }

    /// Show inventory
    fn inventory(&self) -> String {
        if self.player.inventory.is_empty() {
            "You are not carrying anything.".to_string()
        } else {
            let items: Vec<_> = self.player.inventory.iter().map(|i| i.name.clone()).collect();
            format!("You are carrying:\n  {}", items.join("\n  "))
        }
    }

    /// Take an item
    fn take(&mut self, item_name: &str) -> String {
        let room = self.rooms.get_mut(&self.player.current_room).unwrap();
        
        if let Some(pos) = room.items.iter().position(|i| 
            i.id == item_name || i.name.to_lowercase().contains(item_name)
        ) {
            let item = room.items.remove(pos);
            if item.takeable {
                self.player.inventory.push(item.clone());
                self.score += 5;
                format!("You take the {}.", item.name)
            } else {
                room.items.insert(pos, item);
                "You can't take that.".to_string()
            }
        } else {
            "I don't see that here.".to_string()
        }
    }

    /// Drop an item
    fn drop(&mut self, item_name: &str) -> String {
        if let Some(pos) = self.player.inventory.iter().position(|i| 
            i.id == item_name || i.name.to_lowercase().contains(item_name)
        ) {
            let item = self.player.inventory.remove(pos);
            let room = self.rooms.get_mut(&self.player.current_room).unwrap();
            room.items.push(item.clone());
            format!("You drop the {}.", item.name)
        } else {
            "You don't have that.".to_string()
        }
    }

    /// Examine an item
    fn examine(&self, item_name: &str) -> String {
        // Check inventory first
        if let Some(item) = self.player.inventory.iter().find(|i| 
            i.id == item_name || i.name.to_lowercase().contains(item_name)
        ) {
            return format!("{}: {}", item.name, item.description);
        }

        // Check room
        let room = self.rooms.get(&self.player.current_room).unwrap();
        if let Some(item) = room.items.iter().find(|i| 
            i.id == item_name || i.name.to_lowercase().contains(item_name)
        ) {
            return format!("{}: {}", item.name, item.description);
        }

        "I don't see that here.".to_string()
    }

    /// Use an item
    fn use_item(&mut self, item_name: &str) -> String {
        // Check if we have the item
        let has_item = self.player.inventory.iter().any(|i| 
            i.id == item_name || i.name.to_lowercase().contains(item_name)
        );

        if !has_item {
            return "You don't have that.".to_string();
        }

        // Special case: using key in treasure room
        if item_name == "key" || item_name == "brass" {
            if self.player.current_room == "treasure" {
                self.won = true;
                self.score += 50;
                return "You insert the brass key into the padlock. It turns with a satisfying click! The chest opens to reveal piles of gold and jewels!".to_string();
            } else {
                return "The key doesn't seem to fit anything here.".to_string();
            }
        }

        // Special case: using lantern
        if item_name == "lantern" {
            return "You light the lantern. Warm golden light fills the area around you.".to_string();
        }

        format!("You use the {}. Nothing special happens.", item_name)
    }

    /// Show help
    fn help(&self) -> String {
        r#"
TEXT ADVENTURE GAME - HELP
==========================

MOVEMENT:
  n, s, e, w, u, d    Move in a direction
  go <direction>      Go in a direction

ITEMS:
  take <item>         Pick up an item
  drop <item>         Drop an item
  examine <item>      Look at an item
  use <item>          Use an item
  inventory (i)       Show what you're carrying

OTHER:
  look (l)            Look around
  score               Show score and moves
  help (?)            Show this help
  quit (q)            Quit game
  restart             Start over

GOAL: Find the treasure and unlock it!
"#.trim().to_string()
    }

    /// Get current location name
    pub fn current_location(&self) -> String {
        self.rooms.get(&self.player.current_room)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// Check if game is running
    pub fn is_active(&self) -> bool {
        !self.game_over
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

/// Game manager for multiple sessions
pub struct GameManager {
    games: HashMap<String, Game>,
}

impl GameManager {
    /// Create a new game manager
    pub fn new() -> Self {
        Self {
            games: HashMap::new(),
        }
    }

    /// Start a new game
    pub fn start(&mut self, session_id: impl Into<String>) -> String {
        let id = session_id.into();
        let game = Game::new();
        let intro = format!("{}", game.look());
        self.games.insert(id, game);
        format!("🎮 Text Adventure Game Started!\n{}\n\nType 'help' for instructions.", intro)
    }

    /// Process command for a session
    pub fn command(&mut self, session_id: &str, input: &str) -> Option<String> {
        self.games.get_mut(session_id).map(|game| game.process_command(input))
    }

    /// Get game state
    pub fn get(&self, session_id: &str) -> Option<&Game> {
        self.games.get(session_id)
    }

    /// End a game session
    pub fn end(&mut self, session_id: &str) -> bool {
        self.games.remove(session_id).is_some()
    }

    /// Check if session has active game
    pub fn is_active(&self, session_id: &str) -> bool {
        self.games.get(session_id)
            .map(|g| g.is_active())
            .unwrap_or(false)
    }

    /// List active games
    pub fn list_active(&self) -> Vec<&str> {
        self.games.iter()
            .filter(|(_, g)| g.is_active())
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

impl Default for GameManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_creation() {
        let game = Game::new();
        assert_eq!(game.player.current_room, "start");
        assert!(!game.game_over);
        assert_eq!(game.moves, 0);
    }

    #[test]
    fn test_look() {
        let game = Game::new();
        let output = game.look();
        assert!(output.contains("Entrance Hall"));
        assert!(output.contains("brass key"));
    }

    #[test]
    fn test_movement() {
        let mut game = Game::new();
        
        // Go north
        let output = game.process_command("north");
        assert!(output.contains("Long Corridor"));
        
        // Go back south
        let output = game.process_command("south");
        assert!(output.contains("Entrance Hall"));
        
        // Try invalid direction
        let output = game.process_command("west");
        assert!(output.contains("can't go"));
    }

    #[test]
    fn test_take_item() {
        let mut game = Game::new();
        
        // Take the key
        let output = game.process_command("take key");
        assert!(output.contains("take the brass key"));
        
        // Check inventory
        let output = game.process_command("inventory");
        assert!(output.contains("brass key"));
        
        // Key should be gone from room
        let room = game.rooms.get("start").unwrap();
        assert!(room.items.iter().all(|i| i.id != "key"));
    }

    #[test]
    fn test_win_game() {
        let mut game = Game::new();
        
        // Navigate and get items
        game.process_command("take key");
        game.process_command("east");
        game.process_command("up");
        game.process_command("take lantern");
        game.process_command("down");
        game.process_command("west");
        game.process_command("north");
        game.process_command("east");
        
        // Use key to win
        let output = game.process_command("use key");
        assert!(output.contains("CONGRATULATIONS"));
        assert!(game.won);
        assert!(game.game_over);
    }

    #[test]
    fn test_game_manager() {
        let mut manager = GameManager::new();
        
        let output = manager.start("session1");
        assert!(output.contains("Game Started"));
        
        let output = manager.command("session1", "look").unwrap();
        assert!(output.contains("Entrance Hall"));
        
        assert!(manager.is_active("session1"));
        
        manager.end("session1");
        assert!(!manager.is_active("session1"));
    }
}
